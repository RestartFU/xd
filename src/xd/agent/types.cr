require "json"

module Xd
  module Agent
    enum Access
      Plan
      ReadOnly
      Edit
      Full

      def wire_name : String
        case self
        when Plan     then "plan"
        when ReadOnly then "read-only"
        when Edit     then "edit"
        when Full     then "full"
        else               "read-only"
        end
      end

      def self.from_wire(name : String?) : self
        case name
        when "plan"      then Plan
        when "read-only" then ReadOnly
        when "edit"      then Edit
        when "full"      then Full
        else                  ReadOnly
        end
      end

      def label : String
        case self
        when Plan     then "Plan only"
        when ReadOnly then "Read only"
        when Edit     then "Edit files"
        when Full     then "Full access"
        else               "Read only"
        end
      end

      def icon_name : String
        case self
        when Plan     then "view-list-bullet-symbolic"
        when ReadOnly then "changes-prevent-symbolic"
        when Edit     then "document-edit-symbolic"
        when Full     then "changes-allow-symbolic"
        else               "changes-prevent-symbolic"
        end
      end
    end

    enum Effort
      Low
      Medium
      High
      XHigh
      Max
      Ultra
      UltraCode

      def wire_name : String
        case self
        when Low       then "low"
        when Medium    then "medium"
        when High      then "high"
        when XHigh     then "xhigh"
        when Max       then "max"
        when Ultra     then "ultra"
        when UltraCode then "ultracode"
        else                "high"
        end
      end

      def self.from_wire(name : String?) : self
        case name
        when "low"      then Low
        when "medium"   then Medium
        when "high"     then High
        when "xhigh"    then XHigh
        when "max"      then Max
        when "ultra"    then Ultra
        when "ultracode" then UltraCode
        else                 High
        end
      end

      def label : String
        case self
        when Low       then "Low"
        when Medium    then "Medium"
        when High      then "High"
        when XHigh     then "Extra high"
        when Max       then "Max"
        when Ultra     then "Ultra"
        when UltraCode then "UltraCode"
        else                "High"
        end
      end
    end

    enum Transport
      Exec
      CodexAppServer
    end

    record Model,
      id : String,
      display_name : String,
      context_window : UInt64

    class RunSpec
      getter prompt : String
      getter model : String?
      getter system_prompt : String?
      getter resume_session_id : String?
      getter workdir : String?
      getter folder_ids : Array(String)
      getter effort : Effort
      getter access : Access
      getter fast : Bool
      getter claude_mode : Bool

      def initialize(
        @prompt : String,
        @model : String? = nil,
        @system_prompt : String? = nil,
        @resume_session_id : String? = nil,
        @workdir : String? = nil,
        @folder_ids = [] of String,
        @effort = Effort::High,
        @access = Access::ReadOnly,
        @fast = false,
        @claude_mode = false,
      )
      end
    end
  end
end
