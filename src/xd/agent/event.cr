module Xd
  module Agent
    enum EventType
      SessionStarted
      Commands
      TextDelta
      ToolUse
      Usage
      Result
      Error
    end

    class Event
      getter type : EventType
      getter text : String?
      getter session_id : String?
      getter commands : Array(String)?
      getter context_used : UInt64
      getter context_window : UInt64

      def initialize(
        @type : EventType,
        @text : String? = nil,
        @session_id : String? = nil,
        @commands : Array(String)? = nil,
        @context_used : UInt64 = 0_u64,
        @context_window : UInt64 = 0_u64,
      )
      end
    end
  end
end
