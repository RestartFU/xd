require "../storage/store"

module Xd
  module Daemon
    class Search
      MAX_RESULTS    =  40
      SNIPPET_LENGTH = 120

      record Hit,
        message_id : Int64,
        chat_id : String,
        title : String,
        role : String,
        snippet : String

      def initialize(@store : Storage::Store)
      end

      def call(text : String) : Array(Hit)
        query = self.class.fts_query(text)
        return [] of Hit unless query

        @store.search(query, MAX_RESULTS).map do |message|
          chat = @store.get_chat(message.chat_id)
          Hit.new(
            message.id,
            message.chat_id,
            chat.title.presence || "Untitled",
            message.role,
            self.class.snippet(message.content)
          )
        end
      end

      def self.fts_query(text : String) : String?
        terms = text.split(/\s+/).reject(&.empty?).map do |word|
          %("#{word.gsub('"', %(""))}"*)
        end
        query = terms.join(' ')
        query unless query.empty?
      end

      def self.snippet(content : String) : String
        flattened = content.gsub(/[\n\r\t]/, ' ').strip
        return flattened if flattened.size <= SNIPPET_LENGTH

        "#{flattened[0, SNIPPET_LENGTH]}…"
      end
    end
  end
end
