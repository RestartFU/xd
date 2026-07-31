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
          return SubagentTool.build(
            string?(input, "subagent_type"),
            string?(input, "description") || string?(input, "prompt")
          )
        end

        if tool == "collab_tool_call" || tool == "collab_agent_tool_call"
          action = string?(input, "tool")
          if action == "spawn_agent" || action == "spawnAgent"
            return SubagentTool.build(
              string?(input, "task_name") ||
                string?(input, "model") ||
                "Codex",
              string?(input, "prompt")
            )
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
