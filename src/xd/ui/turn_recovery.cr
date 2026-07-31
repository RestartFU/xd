require "json"

module Xd
  module UI
    module TurnRecovery
      extend self

      # Snapshot already contains same-turn start and text/tool events through
      # its watermark. Later events and events from a queued next turn replay.
      # Coalesced text retains per-delta parts so a watermark inside one merged
      # inbox event can trim only the represented prefix.
      def replay(
        event : Hash(String, JSON::Any),
        turn_id : Int64?,
        sequence : Int64?,
      ) : Hash(String, JSON::Any)?
        return event unless turn_id && sequence
        return event unless event["turn_id"]?.try(&.as_i64?) == turn_id

        case event["event"]?.try(&.as_s?)
        when "turn-started"
          nil
        when "text"
          if parts = event["turn_parts"]?.try(&.as_a?)
            text = String.build do |io|
              parts.each do |node|
                part = node.as_h
                if part["sequence"].as_i64 > sequence
                  io << part["text"].as_s
                end
              end
            end
            return if text.empty?

            replay = event.dup
            replay["text"] = JSON::Any.new(text)
            replay
          else
            event_sequence = event["turn_sequence"]?.try(&.as_i64?)
            event if event_sequence.nil? || event_sequence > sequence
          end
        when "tool"
          event_sequence = event["turn_sequence"]?.try(&.as_i64?)
          event if event_sequence.nil? || event_sequence > sequence
        else
          event
        end
      end

      def replay?(
        event : Hash(String, JSON::Any),
        turn_id : Int64?,
        sequence : Int64?,
      ) : Bool
        !replay(event, turn_id, sequence).nil?
      end
    end
  end
end
