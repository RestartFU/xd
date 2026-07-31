require "json"
require "./subagent_tool"
require "./tool_diff"

module Xd
  module Agent
    module ToolSummary
      extend self

      DETAIL_KEYS = %w(
        command file_path filePath path pattern url query description
        notebook_path notebookPath prompt
      )
      FILE_TOOLS = %w(
        edit write multiedit notebookedit apply_patch edit_file write_file patch
      )
      DETAIL_LIMIT = 110

      def changes_files?(name : String?) : Bool
        return false unless name
        FILE_TOOLS.includes?(name.downcase)
      end

      def build(
        name : String?,
        input : Hash(String, JSON::Any)?,
      ) : String
        tool = name || "tool"
        if patch = ToolDiff.build(name, input)
          return patch
        end

        if tool == "Task" || tool == "Agent"
          return claude_subagent(input)
        end

        if tool == "collab_tool_call" || tool == "collab_agent_tool_call"
          action = string?(input, "tool")
          if action == "spawn_agent" || action == "spawnAgent"
            return codex_subagent(input)
          end
        end

        detail = nil
        command = false
        DETAIL_KEYS.each_with_index do |key, index|
          if value = string?(input, key)
            detail = value
            command = index == 0
            break
          end
        end

        stable_name = changes_files?(tool) ? "file_change" : tool
        return stable_name unless detail

        text = command ? unwrap_shell(detail) : detail
        text = text.split.join(" ")
        return stable_name if text.empty?
        text = truncate(text)

        command ? "$ #{text}" : "#{stable_name}  #{text}"
      end

      private def claude_subagent(
        input : Hash(String, JSON::Any)?,
      ) : String
        identity = ["Claude"] of String
        append_unique(identity, string?(input, "subagent_type"))
        append_unique(identity, string?(input, "model"))

        description = string?(input, "description")
        prompt = string?(input, "prompt")
        detail = join_distinct(description, prompt)
        SubagentTool.build(identity.join(" · "), detail)
      end

      private def codex_subagent(
        input : Hash(String, JSON::Any)?,
      ) : String
        identity = ["Codex"] of String
        append_unique(identity, string?(input, "model"))
        append_unique(identity, string?(input, "reasoningEffort"))

        receivers = string_array(input, "receiverThreadIds")
        status, state_message = codex_agent_state(input, receivers)
        detail = [status] of String
        append_unique(detail, string?(input, "prompt"))
        unless receivers.empty?
          append_unique(detail, "Agent #{short_id(receivers.first)}")
        end
        append_unique(detail, state_message)

        SubagentTool.build(identity.join(" · "), detail.join(" · "))
      end

      private def codex_agent_state(
        input : Hash(String, JSON::Any)?,
        receivers : Array(String),
      ) : Tuple(String, String?)
        states = input.try(&.["agentsStates"]?.try(&.as_h?))
        agent = nil
        receivers.each do |thread_id|
          if value = states.try(&.[thread_id]?.try(&.as_h?))
            agent = value
            break
          end
        end
        agent ||= states.try(&.each_value.find(&.as_h?).try(&.as_h?))

        if agent
          status = human_agent_status(string?(agent, "status"))
          return {status, string?(agent, "message")}
        end

        case string?(input, "status")
        when "failed"     then {"Spawn failed", nil}
        when "inProgress" then {"Starting", nil}
        when "completed"  then {"Started", nil}
        else                    {"Delegated", nil}
        end
      end

      private def human_agent_status(status : String?) : String
        case status
        when "pendingInit"  then "Starting"
        when "running"      then "Running"
        when "interrupted"  then "Interrupted"
        when "completed"    then "Completed"
        when "errored"      then "Failed"
        when "shutdown"     then "Stopped"
        when "notFound"     then "Not found"
        else                      "Delegated"
        end
      end

      private def string_array(
        input : Hash(String, JSON::Any)?,
        key : String,
      ) : Array(String)
        values = [] of String
        input.try(&.[key]?.try(&.as_a?)).try do |items|
          items.each do |item|
            if value = item.as_s?
              values << value
            end
          end
        end
        values
      end

      private def join_distinct(first : String?, second : String?) : String?
        parts = [] of String
        append_unique(parts, first)
        append_unique(parts, second)
        return nil if parts.empty?
        parts.join(" · ")
      end

      private def append_unique(parts : Array(String), value : String?) : Nil
        return unless value
        normalized = value.split.join(" ")
        return if normalized.empty?
        return if parts.any? { |part| part.downcase == normalized.downcase }
        parts << normalized
      end

      private def short_id(value : String) : String
        return value if value.size <= 12
        value[0, 12] + "…"
      end

      private def string?(
        input : Hash(String, JSON::Any)?,
        key : String,
      ) : String?
        input.try(&.[key]?.try(&.as_s?))
      end

      private def unwrap_shell(command : String) : String
        space = command.index(' ')
        return command unless space

        program = File.basename(command[0, space])
        return command unless program.ends_with?("sh")

        {" -lic ", " -lc ", " -ic ", " -c "}.each do |flag|
          if at = command.index(flag)
            inner = command[(at + flag.size)..]
            if inner.size >= 2 &&
               {'"', '\''}.includes?(inner[0]) &&
               inner[-1] == inner[0]
              return inner[1, inner.size - 2]
            end
            return inner
          end
        end
        command
      end

      private def truncate(text : String) : String
        return text if text.size <= DETAIL_LIMIT
        text[0, DETAIL_LIMIT] + "…"
      end
    end
  end
end
