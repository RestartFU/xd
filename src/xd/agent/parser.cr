require "json"
require "set"
require "./catalog"
require "./event"
require "./tool_summary"

module Xd
  module Agent
    class Parser
      private class PendingTool
        getter id : String?
        getter name : String?
        property arguments = ""

        def initialize(@id, @name)
        end
      end

      getter backend : Backend
      getter session_id : String?
      property model : String

      @streamed_text = false
      @pending_tools = {} of Int32 => PendingTool
      @deferred_file_tools = {} of String => String
      @started_commands = Set(String).new

      def initialize(@backend : Backend)
        @model = backend.default_model
      end

      # CLI output is not trusted protocol input. Unknown or malformed lines
      # are skipped so one noisy diagnostic cannot end a turn.
      def feed_line(line : String) : Array(Event)
        return [] of Event if line.empty?

        root = JSON.parse(line).as_h?
        return [] of Event unless root

        case @backend.id
        when "claude" then parse_claude(root)
        when "codex"  then parse_codex(root)
        else               [] of Event
        end
      rescue JSON::ParseException
        [] of Event
      end

      private def parse_claude(
        root : Hash(String, JSON::Any),
      ) : Array(Event)
        events = [] of Event

        case string?(root, "type")
        when "system"
          if string?(root, "subtype") == "init"
            if id = string?(root, "session_id")
              @session_id = id
              events << Event.new(
                EventType::SessionStarted,
                session_id: id
              )
            end
            if commands = string_array?(root, "slash_commands")
              commands = commands.compact_map do |command|
                stripped = command.starts_with?('/') ? command[1..] : command
                stripped unless stripped.empty?
              end.first(Event::MAX_COMMANDS)
              unless commands.empty?
                events << Event.new(
                  EventType::Commands,
                  commands: commands
                )
              end
            end
          end
        when "stream_event"
          parse_claude_stream(root, events)
        when "assistant"
          parse_claude_assistant(root, events)
        when "user"
          emit_completed_file_tools(root, events)
        when "result"
          flush_deferred_file_tools(events)
          failed = bool?(root, "is_error") || false
          emit_claude_usage(root, events) unless failed
          events << Event.new(
            failed ? EventType::Error : EventType::Result,
            text: string?(root, "result"),
            session_id: string?(root, "session_id")
          )
        end
        events
      end

      private def parse_claude_stream(
        root : Hash(String, JSON::Any),
        events : Array(Event),
      ) : Nil
        event = object?(root, "event")
        return unless event

        case string?(event, "type")
        when "content_block_delta"
          delta = object?(event, "delta")
          return unless delta

          case string?(delta, "type")
          when "text_delta"
            if text = string?(delta, "text")
              @streamed_text = true
              events << Event.new(EventType::TextDelta, text: text)
            end
          when "input_json_delta"
            index = int?(event, "index")
            fragment = string?(delta, "partial_json")
            if index && fragment && (pending = @pending_tools[index.to_i])
              pending.arguments += fragment
            end
          end
        when "content_block_start"
          block = object?(event, "content_block")
          index = int?(event, "index")
          if block && index && index >= 0 &&
             string?(block, "type") == "tool_use"
            @pending_tools[index.to_i] = PendingTool.new(
              string?(block, "id"),
              string?(block, "name")
            )
          end
        when "content_block_stop"
          index = int?(event, "index")
          return unless index
          pending = @pending_tools.delete(index.to_i)
          return unless pending

          emit_or_defer_tool(
            pending.name,
            pending.id,
            parse_arguments(pending.arguments),
            events
          )
        end
      end

      private def parse_claude_assistant(
        root : Hash(String, JSON::Any),
        events : Array(Event),
      ) : Nil
        message = object?(root, "message")
        content = message.try { |item| array?(item, "content") }
        return unless content

        content.each do |node|
          block = node.as_h?
          next unless block

          case string?(block, "type")
          when "tool_use"
            if @pending_tools.empty?
              emit_or_defer_tool(
                string?(block, "name"),
                string?(block, "id"),
                object?(block, "input"),
                events
              )
            end
          when "text"
            unless @streamed_text
              if text = string?(block, "text")
                events << Event.new(EventType::TextDelta, text: text)
              end
            end
          end
        end
      end

      private def emit_or_defer_tool(
        name : String?,
        id : String?,
        arguments : Hash(String, JSON::Any)?,
        events : Array(Event),
      ) : Nil
        summary = ToolSummary.build(name, arguments)
        if ToolSummary.changes_files?(name) && id
          @deferred_file_tools[id] = summary
        else
          events << Event.new(EventType::ToolUse, text: summary)
        end
      end

      private def emit_completed_file_tools(
        root : Hash(String, JSON::Any),
        events : Array(Event),
      ) : Nil
        message = object?(root, "message")
        content = message.try { |item| array?(item, "content") }
        return unless content

        content.each do |node|
          block = node.as_h?
          next unless block
          next unless string?(block, "type") == "tool_result"
          id = string?(block, "tool_use_id")
          next unless id
          if summary = @deferred_file_tools.delete(id)
            events << Event.new(EventType::ToolUse, text: summary)
          end
        end
      end

      private def flush_deferred_file_tools(
        events : Array(Event),
      ) : Nil
        @deferred_file_tools.each_value do |summary|
          events << Event.new(EventType::ToolUse, text: summary)
        end
        @deferred_file_tools.clear
      end

      private def emit_claude_usage(
        root : Hash(String, JSON::Any),
        events : Array(Event),
      ) : Nil
        usage = object?(root, "usage")
        if usage && (iterations = array?(usage, "iterations")) &&
           !iterations.empty?
          usage = iterations.last.as_h?
        end

        used = usage_total(usage)
        window = claude_context_window(root)
        if used > 0 && window > 0
          events << Event.new(
            EventType::Usage,
            context_used: used,
            context_window: window
          )
        end
      end

      private def usage_total(
        usage : Hash(String, JSON::Any)?,
      ) : UInt64
        return 0_u64 unless usage
        %w(
          input_tokens cache_creation_input_tokens
          cache_read_input_tokens output_tokens
        ).sum(0_u64) do |name|
          positive_u64(usage, name)
        end
      end

      private def claude_context_window(
        root : Hash(String, JSON::Any),
      ) : UInt64
        models = object?(root, "modelUsage")
        models.try do |catalog|
          catalog.each do |name, node|
            model = node.as_h?
            next unless model
            canonical = string?(model, "canonicalModel")
            next unless name == @model ||
                        canonical == @model ||
                        name.starts_with?(@model)
            window = positive_u64(model, "contextWindow")
            return window if window > 0
          end
        end
        @backend.context_window(@model)
      end

      private def parse_codex(
        root : Hash(String, JSON::Any),
      ) : Array(Event)
        events = [] of Event

        case string?(root, "type")
        when "thread.started"
          if id = string?(root, "thread_id")
            @session_id = id
            events << Event.new(
              EventType::SessionStarted,
              session_id: id
            )
          end
        when "item.started"
          item = object?(root, "item")
          if item && string?(item, "type") == "command_execution"
            summary = ToolSummary.build("command_execution", item)
            events << Event.new(EventType::ToolUse, text: summary)
            if id = string?(item, "id")
              @started_commands << id
            end
          end
        when "item.completed"
          item = object?(root, "item")
          return events unless item
          id = string?(item, "id")
          return events if id && @started_commands.delete(id)
          parse_codex_item(item, events)
        when "turn.completed"
          emit_codex_usage(root, events)
          events << Event.new(EventType::Result)
        when "turn.failed", "error"
          error = object?(root, "error")
          message = string?(root, "message") ||
                    error.try { |item| string?(item, "message") } ||
                    "The turn failed"
          events << Event.new(EventType::Error, text: message)
        end
        events
      end

      private def parse_codex_item(
        item : Hash(String, JSON::Any),
        events : Array(Event),
      ) : Nil
        type = string?(item, "type")
        if type == "agent_message"
          if text = string?(item, "text")
            @streamed_text = true
            events << Event.new(EventType::TextDelta, text: text)
          end
        else
          events << Event.new(
            EventType::ToolUse,
            text: ToolSummary.build(type, item)
          )
        end
      end

      private def emit_codex_usage(
        root : Hash(String, JSON::Any),
        events : Array(Event),
      ) : Nil
        usage = object?(root, "usage")
        return unless usage

        used = positive_u64(usage, "input_tokens") +
               positive_u64(usage, "output_tokens")
        window = @backend.context_window(@model)
        if rollout = rollout_context
          used, window = rollout
        end
        events << Event.new(
          EventType::Usage,
          context_used: used,
          context_window: window
        )
      end

      private def rollout_context : Tuple(UInt64, UInt64)?
        id = @session_id
        return nil unless id

        home = ENV["CODEX_HOME"]? || File.join(Path.home, ".codex")
        path = find_rollout(File.join(home, "sessions"), id, 0)
        return nil unless path

        size = File.size(path)
        offset = Math.max(size - 1_048_576, 0_i64)
        used = 0_u64
        window = 0_u64

        File.open(path) do |file|
          file.seek(offset)
          file.gets if offset > 0
          file.each_line do |line|
            next unless line.includes?("\"token_count\"")
            begin
              root = JSON.parse(line).as_h?
              payload = root.try { |item| object?(item, "payload") }
              next unless payload
              next unless string?(payload, "type") == "token_count"
              info = object?(payload, "info")
              usage = info.try { |item| object?(item, "last_token_usage") }
              next unless info && usage
              used = positive_u64(usage, "total_tokens")
              window = positive_u64(info, "model_context_window")
            rescue JSON::ParseException
            end
          end
        end
        used > 0 && window > 0 ? {used, window} : nil
      rescue File::Error | IO::Error
        nil
      end

      private def find_rollout(
        directory : String,
        thread_id : String,
        depth : Int32,
      ) : String?
        return nil if depth > 4
        return nil unless Dir.exists?(directory)

        suffix = "#{thread_id}.jsonl"
        Dir.each_child(directory) do |name|
          path = File.join(directory, name)
          if File.directory?(path)
            if found = find_rollout(path, thread_id, depth + 1)
              return found
            end
          elsif name.ends_with?(suffix)
            return path
          end
        end
        nil
      rescue File::Error
        nil
      end

      private def parse_arguments(
        text : String,
      ) : Hash(String, JSON::Any)?
        return nil if text.empty?
        JSON.parse(text).as_h?
      rescue JSON::ParseException
        nil
      end

      private def object?(
        root : Hash(String, JSON::Any),
        name : String,
      ) : Hash(String, JSON::Any)?
        root[name]?.try(&.as_h?)
      end

      private def array?(
        root : Hash(String, JSON::Any),
        name : String,
      ) : Array(JSON::Any)?
        root[name]?.try(&.as_a?)
      end

      private def string_array?(
        root : Hash(String, JSON::Any),
        name : String,
      ) : Array(String)?
        array?(root, name).try do |items|
          items.compact_map(&.as_s?)
        end
      end

      private def string?(
        root : Hash(String, JSON::Any),
        name : String,
      ) : String?
        root[name]?.try(&.as_s?)
      end

      private def int?(
        root : Hash(String, JSON::Any),
        name : String,
      ) : Int64?
        root[name]?.try(&.as_i64?)
      end

      private def bool?(
        root : Hash(String, JSON::Any),
        name : String,
      ) : Bool?
        root[name]?.try(&.as_bool?)
      end

      private def positive_u64(
        root : Hash(String, JSON::Any),
        name : String,
      ) : UInt64
        value = int?(root, name) || 0_i64
        Math.max(value, 0_i64).to_u64
      end
    end
  end
end
