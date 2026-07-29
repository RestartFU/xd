require "json"

module Xd
  module Storage
    class Chat
      getter id : String
      getter folder_id : String
      getter title : String?
      getter backend : String
      getter workdir : String?
      getter original_workdir : String?
      getter model : String?
      getter effort : String?
      getter access : String?
      getter queue : Array(String)
      getter new_worktree : Bool
      getter plan : Bool
      getter terminal_open : Bool
      getter diff_open : Bool
      getter daemon_working : Bool
      getter created_at : Int64
      getter updated_at : Int64
      property context_used : UInt64 = 0_u64
      property context_window : UInt64 = 0_u64

      def initialize(
        @id : String,
        @folder_id : String,
        @title : String?,
        @backend : String,
        @workdir : String?,
        @model : String?,
        @effort : String?,
        @access : String?,
        @plan : Bool,
        @created_at : Int64,
        @updated_at : Int64,
        @terminal_open : Bool,
        @diff_open : Bool,
        @queue : Array(String),
        @new_worktree : Bool,
        @original_workdir : String?,
        @daemon_working : Bool,
      )
      end
    end

    def self.queue_from_column(stored : Nil) : Array(String)
      [] of String
    end

    def self.queue_from_column(stored : String) : Array(String)
      return [] of String if stored.empty?

      begin
        parsed = JSON.parse(stored)
        return [stored] unless parsed.raw.is_a?(Array)

        parsed.as_a.compact_map do |entry|
          value = entry.as_s?
          value if value && !value.empty?
        end
      rescue JSON::ParseException
        [stored]
      end
    end

    def self.queue_to_column(messages : Array(String)) : String?
      messages.empty? ? nil : messages.to_json
    end
  end
end
