require "json"
require "set"
require "./catalog"
require "./event"
require "./tool_summary"

module Xd
  module Agent
    class CodexTurn
      getter spec : RunSpec
      getter allowed_environment_names : Array(String)?
      property thread_id : String?
      property turn_id : String?
      property backend_error : String?
      property cancel_callback : Proc(Nil)? = nil
      property stopping = false
      property finished = false
      property latest_diff : String?
      property emitted_file_diff = false
      getter streamed_messages = Set(String).new
      getter started_commands = Set(String).new

      def initialize(
        @spec : RunSpec,
        @allowed_environment_names : Array(String)?,
        @on_event : Proc(Event, Nil),
        @on_finished : Proc(Bool, String?, Nil),
      )
        @latest_diff = nil
      end

      def emit(
        type : EventType,
        text : String? = nil,
        used : UInt64 = 0_u64,
        window : UInt64 = 0_u64,
      ) : Nil
        @on_event.call(Event.new(
          type,
          text: text,
          session_id: type.session_started? ? @thread_id : nil,
          context_used: used,
          context_window: window
        ))
      end

      def finish(success : Bool, message : String?) : Nil
        return if @finished
        @finished = true
        @on_finished.call(success, message)
      end

      def cancel : Nil
        @cancel_callback.try(&.call)
      end
    end

    class CodexProtocol
      enum RequestKind
        Initialize
        OpenThread
        StartTurn
        Interrupt
      end

      private record PendingRequest,
        kind : RequestKind,
        turn : CodexTurn?

      getter ready = false
      getter failed = false

      @next_id = 0_i64
      @pending = {} of Int64 => PendingRequest
      @turns = {} of String => CodexTurn
      @waiting = [] of CodexTurn

      def initialize(
        @backend : Backend,
        @version : String,
        @writer : Proc(String, Nil),
      )
      end

      def initialize_client : Nil
        params = {
          "clientInfo" => json_any({
            "name"    => "xd",
            "title"   => "xd",
            "version" => @version,
          }),
        }
        request("initialize", params, RequestKind::Initialize, nil)
      end

      def start_turn(
        spec : RunSpec,
        allowed_environment_names : Array(String)?,
        on_event : Proc(Event, Nil),
        on_finished : Proc(Bool, String?, Nil),
      ) : CodexTurn
        turn = CodexTurn.new(
          spec,
          allowed_environment_names,
          on_event,
          on_finished
        )
        if @failed
          turn.finish(false, "Codex app-server failed")
        elsif @ready
          open_turn(turn)
        else
          @waiting << turn
        end
        turn
      end

      def cancel(turn : CodexTurn) : Nil
        return if turn.finished || turn.stopping
        turn.stopping = true
        interrupt_turn(turn)
      end

      # Cancellation timeout owns local completion. Server is discarded by
      # caller afterward, so no late notification may revive this turn.
      def complete_cancel(turn : CodexTurn) : Nil
        @waiting.delete(turn)
        if id = turn.thread_id
          @turns.delete(id)
        end
        stale = @pending.compact_map do |id, request|
          id if request.turn.try(&.same?(turn))
        end
        stale.each { |id| @pending.delete(id) }
        turn.finish(true, nil)
      end

      def receive_line(line : String) : Nil
        root = JSON.parse(line).as_h?
        return unless root

        if root.has_key?("id") &&
           (root.has_key?("result") || root.has_key?("error"))
          handle_response(root)
        else
          handle_notification(root)
        end
      rescue JSON::ParseException
      end

      def fail(message : String) : Nil
        return if @failed
        @failed = true

        unique = Set(CodexTurn).new
        @waiting.each { |turn| unique << turn }
        @turns.each_value { |turn| unique << turn }
        @pending.each_value do |request|
          if turn = request.turn
            unique << turn
          end
        end
        @waiting.clear
        @turns.clear
        @pending.clear
        unique.each { |turn| turn.finish(false, message) }
      end

      private def request(
        method : String,
        params : Hash(String, JSON::Any),
        kind : RequestKind,
        turn : CodexTurn?,
      ) : Int64
        @next_id += 1
        id = @next_id
        @pending[id] = PendingRequest.new(kind, turn)
        write({
          "id"     => JSON::Any.new(id),
          "method" => JSON::Any.new(method),
          "params" => JSON::Any.new(params),
        })
        id
      end

      private def notify(
        method : String,
        params = {} of String => JSON::Any,
      ) : Nil
        write({
          "method" => JSON::Any.new(method),
          "params" => JSON::Any.new(params),
        })
      end

      private def write(root : Hash(String, JSON::Any)) : Nil
        return if @failed
        @writer.call(root.to_json + "\n")
      end

      private def open_turn(turn : CodexTurn) : Nil
        spec = turn.spec
        params = {
          "approvalPolicy" => JSON::Any.new("never"),
          "sandbox"        => JSON::Any.new(
            case spec.access
            when Access::Full then "danger-full-access"
            when Access::Edit then "workspace-write"
            else                   "read-only"
            end
          ),
        }
        if model = spec.model
          params["model"] = JSON::Any.new(model)
        end
        if workdir = spec.workdir
          params["cwd"] = JSON::Any.new(workdir)
        end
        if instructions = @backend.developer_instructions(spec)
          params["developerInstructions"] = JSON::Any.new(instructions)
        end
        if names = turn.allowed_environment_names
          policy = {
            "inherit"                 => JSON::Any.new("all"),
            "ignore_default_excludes" => JSON::Any.new(true),
            "include_only"            => JSON::Any.new(
              names.map { |name| JSON::Any.new(name) }
            ),
          }
          params["config"] = json_any({
            "shell_environment_policy" => policy,
          })
        end

        if id = spec.resume_session_id
          params["threadId"] = JSON::Any.new(id)
          request(
            "thread/resume",
            params,
            RequestKind::OpenThread,
            turn
          )
        else
          request(
            "thread/start",
            params,
            RequestKind::OpenThread,
            turn
          )
        end
      end

      private def start_turn_request(turn : CodexTurn) : Nil
        thread_id = turn.thread_id
        unless thread_id
          finish_turn(turn, false, "Codex app-server returned no thread id")
          return
        end

        spec = turn.spec
        sandbox = {
          "type" => JSON::Any.new(@backend.sandbox_policy(spec.access)),
        }
        if spec.access.edit? && (workdir = spec.workdir)
          sandbox["writableRoots"] = JSON::Any.new([
            JSON::Any.new(workdir),
          ])
          sandbox["networkAccess"] = JSON::Any.new(false)
        end

        input = [
          {"type" => "text", "text" => spec.prompt},
        ]
        if audio_path = spec.audio_path
          input << {
            "type" => "localAudio",
            "path" => audio_path,
          }
        end

        params = {
          "threadId"       => JSON::Any.new(thread_id),
          "approvalPolicy" => JSON::Any.new("never"),
          "effort"         => JSON::Any.new(spec.effort.wire_name),
          "sandboxPolicy"  => JSON::Any.new(sandbox),
          "input"          => json_any(input),
        }
        if model = spec.model
          params["model"] = JSON::Any.new(model)
        end
        if workdir = spec.workdir
          params["cwd"] = JSON::Any.new(workdir)
        end
        request(
          "turn/start",
          params,
          RequestKind::StartTurn,
          turn
        )
      end

      private def interrupt_turn(turn : CodexTurn) : Nil
        thread_id = turn.thread_id
        turn_id = turn.turn_id
        return unless thread_id && turn_id && !turn.finished

        request(
          "turn/interrupt",
          {
            "threadId" => JSON::Any.new(thread_id),
            "turnId"   => JSON::Any.new(turn_id),
          },
          RequestKind::Interrupt,
          turn
        )
      end

      private def handle_response(
        root : Hash(String, JSON::Any),
      ) : Nil
        id = int?(root, "id") || 0_i64
        pending = @pending.delete(id)
        return unless pending

        if error = object?(root, "error")
          message = string?(error, "message")
          if pending.kind.initialize?
            fail(
              message ||
              "Codex app-server initialization failed"
            )
          elsif turn = pending.turn
            finish_turn(
              turn,
              false,
              message || "Codex app-server request failed"
            )
          end
          return
        end

        result = object?(root, "result")
        case pending.kind
        when RequestKind::Initialize
          @ready = true
          notify("initialized")
          waiting = @waiting
          @waiting = [] of CodexTurn
          waiting.each { |turn| open_turn(turn) }
        when RequestKind::OpenThread
          turn = pending.turn
          return unless turn
          thread = result.try { |item| object?(item, "thread") }
          thread_id = thread.try { |item| string?(item, "id") }
          unless thread_id
            finish_turn(
              turn,
              false,
              "Codex app-server returned no thread id"
            )
            return
          end
          turn.thread_id = thread_id
          @turns[thread_id] = turn
          turn.emit(EventType::SessionStarted)
          start_turn_request(turn)
        when RequestKind::StartTurn
          turn = pending.turn
          return unless turn
          started = result.try { |item| object?(item, "turn") }
          if turn_id = started.try { |item| string?(item, "id") }
            turn.turn_id = turn_id
          end
          interrupt_turn(turn) if turn.stopping
        when RequestKind::Interrupt
        end
      end

      private def handle_notification(
        root : Hash(String, JSON::Any),
      ) : Nil
        method = string?(root, "method")
        params = object?(root, "params")
        return unless method && params

        thread_id = string?(params, "threadId")
        turn = thread_id.try { |id| @turns[id]? }

        case method
        when "item/agentMessage/delta"
          return unless turn
          if id = string?(params, "itemId")
            turn.streamed_messages << id
          end
          if delta = string?(params, "delta")
            turn.emit(EventType::TextDelta, delta)
          end
        when "item/started"
          handle_item(turn, params, true) if turn
        when "item/completed"
          handle_item(turn, params, false) if turn
        when "thread/tokenUsage/updated"
          return unless turn
          usage = object?(params, "tokenUsage")
          last = usage.try { |item| object?(item, "last") }
          used = last ? positive_u64(last, "totalTokens") : 0_u64
          window = usage ? positive_u64(usage, "modelContextWindow") : 0_u64
          turn.emit(EventType::Usage, used: used, window: window)
        when "turn/diff/updated"
          return unless turn
          turn.latest_diff = string?(params, "diff")
        when "error"
          return unless turn
          error = object?(params, "error")
          message = error.try { |item| string?(item, "message") }
          retrying = bool?(params, "willRetry") || false
          if !retrying && message && !turn.stopping
            turn.backend_error = message
          end
        when "turn/completed"
          return unless turn
          completed = object?(params, "turn")
          status = completed.try { |item| string?(item, "status") }
          error = completed.try { |item| object?(item, "error") }
          message = error.try { |item| string?(item, "message") }

          if turn.stopping || status == "interrupted"
            finish_turn(turn, true, nil)
          elsif status == "failed"
            finish_turn(
              turn,
              false,
              message || turn.backend_error
            )
          elsif status == "completed"
            emit_pending_diff(turn)
            turn.emit(EventType::Result)
            finish_turn(
              turn,
              turn.backend_error.nil?,
              turn.backend_error
            )
          else
            finish_turn(
              turn,
              false,
              "Codex returned an unknown turn status"
            )
          end
        when "item/commandExecution/requestApproval",
             "item/fileChange/requestApproval"
          if id = int?(root, "id")
            write({
              "id"     => JSON::Any.new(id),
              "result" => json_any({
                "decision" => "cancel",
              }),
            })
          end
        else
          if id = int?(root, "id")
            write({
              "id"    => JSON::Any.new(id),
              "error" => json_any({
                "code"    => -32601,
                "message" => "xd does not support this server request",
              }),
            })
          end
        end
      end

      private def handle_item(
        turn : CodexTurn,
        params : Hash(String, JSON::Any),
        started : Bool,
      ) : Nil
        item = object?(params, "item")
        return unless item
        type = string?(item, "type")
        id = string?(item, "id")
        return unless type

        if type == "agentMessage"
          if !started && id &&
             !turn.streamed_messages.includes?(id) &&
             (text = string?(item, "text"))
            turn.emit(EventType::TextDelta, text)
          end
          return
        end

        return if {
                    "userMessage",
                    "reasoning",
                    "hookPrompt",
                    "subAgentActivity",
                  }.includes?(type)

        if type == "commandExecution"
          return unless started && id
          return unless turn.started_commands.add?(id)
        elsif started
          return
        end

        summary_name = case type
                       when "commandExecution"    then "command_execution"
                       when "fileChange"          then "file_change"
                       when "mcpToolCall"         then "mcp_tool_call"
                       when "collabAgentToolCall" then "collab_agent_tool_call"
                       when "webSearch"           then "web_search"
                       when "imageView"           then "image_view"
                       else                            type
                       end
        turn.emit(
          EventType::ToolUse,
          tool_summary(turn, summary_name, item)
        )
      end

      private def tool_summary(
        turn : CodexTurn,
        name : String,
        item : Hash(String, JSON::Any),
      ) : String
        summary = ToolSummary.build(name, item)
        return summary unless name == "file_change"

        if summary.starts_with?(ToolDiff::PREFIX)
          turn.emitted_file_diff = true
          summary
        elsif fallback = ToolDiff.wrap_unified(turn.latest_diff)
          turn.emitted_file_diff = true
          fallback
        else
          summary
        end
      end

      private def finish_turn(
        turn : CodexTurn,
        success : Bool,
        message : String?,
      ) : Nil
        emit_pending_diff(turn)
        if id = turn.thread_id
          @turns.delete(id)
        end
        turn.finish(success, message)
      end

      private def emit_pending_diff(turn : CodexTurn) : Nil
        return if turn.emitted_file_diff
        return unless summary = ToolDiff.wrap_unified(turn.latest_diff)

        turn.emitted_file_diff = true
        turn.emit(EventType::ToolUse, summary)
      end

      private def object?(
        root : Hash(String, JSON::Any),
        name : String,
      ) : Hash(String, JSON::Any)?
        root[name]?.try(&.as_h?)
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

      private def json_any(value) : JSON::Any
        JSON.parse(value.to_json)
      end
    end
  end
end
