require "json"
require "set"
require "../storage/workflow_state"
require "../workspace/service"
require "../workspace/worktrees"
require "./catalog"
require "./codex_app_server"
require "./conversation"
require "./environment"
require "./exec_session"
require "./executable"
require "./git_diff_tracker"
require "./ask"
require "./secrets"
require "./workflow_run"
require "./workspace_block"

module Xd
  module Agent
    abstract class SessionHandle
      abstract def cancel : Nil
    end

    class CallbackHandle < SessionHandle
      def initialize(@callback : Proc(Nil))
      end

      def cancel : Nil
        @callback.call
      end
    end

    abstract class Launcher
      abstract def start(
        backend : Backend,
        spec : RunSpec,
        environment : Hash(String, String),
        secret_names : Array(String),
        on_event : Proc(Event, Nil),
        on_finished : Proc(Bool, String?, Nil),
      ) : SessionHandle

      def close : Nil
      end

      def reload(provider : String) : Nil
      end
    end

    class ProcessLauncher < Launcher
      def initialize(version : String)
        @codex = CodexPool.new(version: version)
      end

      def start(
        backend : Backend,
        spec : RunSpec,
        environment : Hash(String, String),
        secret_names : Array(String),
        on_event : Proc(Event, Nil),
        on_finished : Proc(Bool, String?, Nil),
      ) : SessionHandle
        case backend.transport
        when Transport::Exec
          arguments = backend.build_argv(spec)
          arguments[0] = Executable.resolve(backend.program)
          session = ExecSession.new(
            backend,
            spec,
            environment,
            on_event,
            on_finished,
            arguments
          )
          session.start
          CallbackHandle.new(-> { session.cancel })
        when Transport::CodexAppServer
          turn = @codex.start(
            spec,
            environment,
            secret_names,
            on_event,
            on_finished
          )
          CallbackHandle.new(-> { turn.cancel })
        else
          raise ArgumentError.new(
            "Unsupported transport for #{backend.display_name}"
          )
        end
      end

      def close : Nil
        @codex.close
      end

      def reload(provider : String) : Nil
        @codex.close if provider == "codex"
      end
    end

    enum SendResult
      Started
      Queued
    end

    record TurnItem, text : String, tool : Bool

    record TurnSnapshot,
      label : String,
      working_for : Int64,
      segment : String,
      items : Array(TurnItem)

    class Manager
      class Error < Exception
      end

      alias Authorizer = Proc(String, String?)

      private class ActiveTurn
        getter chat_id : String
        getter backend : Backend
        getter model : String
        getter effort : Effort
        getter prompt : String
        getter resumed : Bool
        getter retry_attempt : Bool
        getter label : String
        property workdir : String
        getter transcript_message_id : Int64
        getter started_at : Time::Instant
        property handle : SessionHandle?
        property cancel_requested = false
        property finished = false
        property commands = [] of String
        property segment = ""
        property segment_message_id = 0_i64
        property visible_segment_bytes = 0
        property diff_tracker : GitDiffTracker?
        property context_used = 0_u64
        property context_window = 0_u64
        property had_text = false
        property had_tool = false

        def initialize(
          @chat_id,
          @backend,
          @model,
          @effort,
          @prompt,
          @resumed,
          @retry_attempt,
          @label,
          @workdir,
          @transcript_message_id,
          @started_at,
        )
          @diff_tracker = GitDiffTracker.open(@workdir)
        end
      end

      @turns = {} of String => ActiveTurn
      @starting = Set(String).new
      @updating = Set(String).new
      @command_sets = {} of String => Array(String)
      @mutex = Mutex.new
      @closed = false
      @worktree_service : Workspace::Worktrees

      def initialize(
        @store : Storage::Store,
        @workspaces : Workspace::Service,
        @launcher : Launcher = ProcessLauncher.new("unknown"),
        @on_event : Proc(String, Hash(String, JSON::Any), Nil) = ->(_name : String, _fields : Hash(String, JSON::Any)) { },
        worktree_service : Workspace::Worktrees? = nil,
        @clock : Proc(Time::Instant) = -> { Time.instant },
        @authorizer : Authorizer = ->(_provider : String) : String? { nil },
      )
        @worktree_service = worktree_service ||
                            Workspace::Worktrees.new(@store, @workspaces)
      end

      def send(chat_id : String, text : String) : SendResult
        if text.empty?
          raise Error.new("A message needs a chat and something to say.")
        end

        queued = @mutex.synchronize do
          raise Error.new("The daemon is stopping.") if @closed

          if @turns.has_key?(chat_id) || @starting.includes?(chat_id)
            @store.queue_append(chat_id, text)
            true
          else
            @starting << chat_id
            false
          end
        end

        if queued
          publish_queue(chat_id)
          return SendResult::Queued
        end

        start_turn(chat_id, text)
        SendResult::Started
      rescue error : Error
        raise error
      rescue error
        fail_start(chat_id, error)
      end

      def cancel(
        chat_id : String,
        publish_queue_event : Bool = true,
      ) : Nil
        handle : SessionHandle? = nil
        queued : String? = nil

        @mutex.synchronize do
          raise Error.new("The daemon is stopping.") if @closed

          if turn = @turns[chat_id]?
            if current = turn.handle
              handle = current
            else
              turn.cancel_requested = true
            end
          elsif !@starting.includes?(chat_id)
            queued = @store.queue_take_first(chat_id)
            @starting << chat_id if queued
          end
        end

        if text = queued
          publish_queue(chat_id) if publish_queue_event
          start_turn(chat_id, text)
        else
          handle.try(&.cancel)
        end
      rescue error : Error
        raise error
      rescue error
        fail_start(chat_id, error)
      end

      def running?(chat_id : String) : Bool
        @mutex.synchronize do
          @turns.has_key?(chat_id) || @starting.includes?(chat_id)
        end
      end

      # Durable transcript revision captured after storing the user message
      # that started this turn. Rows written while the agent is running are
      # replayed through semantic events, so snapshot readers stop here and
      # never draw those live rows twice.
      def transcript_message_id(chat_id : String) : Int64?
        @mutex.synchronize do
          @turns[chat_id]?.try(&.transcript_message_id)
        end
      end

      def active_turn(chat_id : String) : TurnSnapshot?
        @mutex.synchronize do
          turn = @turns[chat_id]?
          next unless turn

          visible_bytes = Math.min(
            Ask.visible_bytes(turn.segment),
            WorkspaceBlock.visible_bytes(turn.segment)
          )
          segment = turn.segment.byte_slice(0, visible_bytes)
          items = @store.list_messages_since(
            chat_id,
            turn.transcript_message_id
          ).compact_map do |message|
            next if message.id == turn.segment_message_id
            next unless message.role == "assistant" ||
                        message.role == "tool"

            TurnItem.new(message.content, message.role == "tool")
          end
          elapsed = Math.max(
            (@clock.call - turn.started_at).total_seconds.to_i64,
            0_i64
          )
          TurnSnapshot.new(turn.label, elapsed, segment, items)
        end
      end

      def commands(chat_id : String) : Array(String)
        backend = @store.get_chat(chat_id).backend
        @mutex.synchronize do
          @command_sets[backend]?.try(&.dup) || [] of String
        end
      end

      # Detach a chat before its database row is deleted. Agent completion may
      # arrive later; removing ownership first makes that callback a no-op.
      def forget(chat_id : String) : Nil
        handle = @mutex.synchronize do
          @starting.delete(chat_id)
          turn = @turns.delete(chat_id)
          if turn
            turn.finished = true
            turn.handle
          end
        end
        handle.try(&.cancel)
      end

      def close : Nil
        turns = @mutex.synchronize do
          return if @closed
          @closed = true
          current = @turns.values.dup
          @turns.clear
          @starting.clear
          @updating.clear
          current
        end

        turns.each do |turn|
          @store.set_daemon_working(turn.chat_id, false)
          turn.handle.try(&.cancel)
        rescue Storage::Error
        end
        @launcher.close
      end

      def begin_backend_update(provider : String) : Nil
        unless Catalog.lookup(provider)
          raise Error.new("Unknown assistant: #{provider}")
        end
        @mutex.synchronize do
          if !@turns.empty? || !@starting.empty?
            raise Error.new(
              "Stop active assistant turns before updating bundled CLIs."
            )
          end
          if @updating.includes?(provider)
            raise Error.new("That assistant is already updating.")
          end
          @updating << provider
        end
      end

      def finish_backend_update(provider : String, success : Bool) : Nil
        reload = @mutex.synchronize do
          @updating.delete(provider)
          success && !@closed
        end
        @launcher.reload(provider) if reload
      end

      private def start_turn(
        chat_id : String,
        text : String,
        user_submitted : Bool = true,
        retry_attempt : Bool = false,
      ) : Nil
        input_stored = !user_submitted
        begin
          chat = @store.get_chat(chat_id)
          backend = Catalog.lookup(chat.backend)
          unless backend
            raise Error.new("Unknown backend \"#{chat.backend}\".")
          end
          updating = @mutex.synchronize { @updating.includes?(backend.id) }
          if updating
            raise Error.new(
              "#{backend.display_name} is updating. Try again when it finishes."
            )
          end
          if message = @authorizer.call(backend.id)
            raise Error.new(message)
          end

          if chat.title == "New Chat" &&
             @store.last_message_id(chat_id) == 0 &&
             (title = Conversation.title(text))
            @store.set_chat_title(chat_id, title)
            publish("tree")
          end

          workdir = @worktree_service.prepare(
            chat,
            Conversation.title(text)
          )
          if user_submitted
            @store.append_message(chat_id, "user", text)
            input_stored = true
          end
          transcript_message_id = @store.last_message_id(chat_id)
          last_seen = @store.get_last_seen(chat_id, backend.id)
          resume_session_id =
            @store.get_session_id(chat_id, backend.id)
          prompt = Conversation.join(
            Conversation.handover(@store, chat_id, last_seen),
            text
          )

          settings = @workspaces.resolve(chat.folder_id, chat.backend)
          folder_ids = @workspaces.folder_ids(chat.folder_id)
          model = chat.model || settings.model || backend.default_model
          effort = chat.effort ? Effort.from_wire(chat.effort) : backend.default_effort
          access = chat.plan ? Access::Plan : Access.from_wire(chat.access)
          secrets = Secrets.effective(folder_ids)
          system_prompt = [
            @workspaces.describe_place(chat.folder_id, workdir),
            settings.instructions,
            Ask::INSTRUCTIONS,
            secrets.prompt,
          ].compact.reject(&.empty?).join("\n\n")
          system_prompt = nil if system_prompt.empty?
          spec = RunSpec.new(
            prompt,
            model: model,
            system_prompt: system_prompt,
            resume_session_id: resume_session_id,
            workdir: workdir,
            folder_ids: folder_ids,
            effort: effort,
            access: access
          )
          turn = ActiveTurn.new(
            chat_id,
            backend,
            model,
            effort,
            text,
            !resume_session_id.nil?,
            retry_attempt,
            "#{backend.model_label(model)} · #{effort.label}",
            workdir,
            transcript_message_id,
            @clock.call
          )

          @mutex.synchronize do
            raise Error.new("The daemon is stopping.") if @closed
            @starting.delete(chat_id)
            @turns[chat_id] = turn
          end

          environment = secrets.environment(Environment.host)
          if backend.id == "claude"
            # App releases own bundled CLI updates. Letting Claude replace
            # itself would make installed bytes diverge from signed bundles.
            environment["DISABLE_AUTOUPDATER"] = "1"
          end
          handle = @launcher.start(
            backend,
            spec,
            environment,
            secrets.names,
            ->(event : Event) { receive(turn, event) },
            ->(ok : Bool, message : String?) { finish(turn, ok, message) }
          )

          cancel = @mutex.synchronize do
            current = @turns[chat_id]?
            if current.same?(turn) && !turn.finished
              turn.handle = handle
              turn.cancel_requested
            else
              true
            end
          end
          if cancel
            handle.cancel
            return
          end

          @store.set_daemon_working(chat_id, true)
          publish("turn-started", {
            "chat"  => JSON::Any.new(chat_id),
            "label" => JSON::Any.new(turn.label),
          })
        rescue error : Error
          cleanup_failed_start(chat_id, input_stored, error.message)
          raise error
        rescue error
          cleanup_failed_start(chat_id, input_stored, error.message)
          raise Error.new(error.message || "Cannot start the agent")
        end
      end

      private def cleanup_failed_start(
        chat_id : String,
        input_stored : Bool,
        message : String?,
      ) : Nil
        @mutex.synchronize do
          @starting.delete(chat_id)
          @turns.delete(chat_id)
        end
        if input_stored && message
          @store.append_message(chat_id, "error", message)
          publish("changed", {"chat" => JSON::Any.new(chat_id)})
        end
        @store.set_daemon_working(chat_id, false)
      rescue Storage::Error
      end

      private def fail_start(chat_id : String, error : Exception) : NoReturn
        @mutex.synchronize do
          @starting.delete(chat_id)
          @turns.delete(chat_id)
        end
        raise Error.new(error.message || "Cannot start the agent")
      end

      private def receive(turn : ActiveTurn, event : Event) : Nil
        event_name : String? = nil
        fields = {} of String => JSON::Any

        @mutex.synchronize do
          current = @turns[turn.chat_id]?
          return unless current.same?(turn) && !turn.finished

          if session_id = event.session_id
            @store.set_session_id(
              turn.chat_id,
              turn.backend.id,
              session_id
            )
          end

          case event.type
          when EventType::Commands
            commands = event.commands || [] of String
            turn.commands = commands.dup
            @command_sets[turn.backend.id] = commands.dup
            event_name = "commands"
            fields = {
              "chat"     => JSON::Any.new(turn.chat_id),
              "backend"  => JSON::Any.new(turn.backend.id),
              "commands" => json_any(commands),
            }
          when EventType::TextDelta
            text = event.text || ""
            return if text.empty?

            turn.had_text = true
            turn.segment += text
            visible_bytes = Math.min(
              Ask.visible_bytes(turn.segment),
              WorkspaceBlock.visible_bytes(turn.segment)
            )
            visible_text = if visible_bytes > turn.visible_segment_bytes
                             turn.segment.byte_slice(
                               turn.visible_segment_bytes,
                               visible_bytes - turn.visible_segment_bytes
                             )
                           else
                             ""
                           end
            turn.visible_segment_bytes = visible_bytes
            if turn.segment_message_id == 0
              turn.segment_message_id = @store.append_message(
                turn.chat_id,
                "assistant",
                turn.segment,
                label: turn.label
              )
            else
              @store.update_message(
                turn.segment_message_id,
                turn.segment
              )
            end
            unless visible_text.empty?
              event_name = "text"
              fields = {
                "chat" => JSON::Any.new(turn.chat_id),
                "text" => JSON::Any.new(visible_text),
              }
            end
          when EventType::ToolUse
            turn.had_tool = true
            close_segment(turn)
            text = event.text || "Used a tool"
            text = turn.diff_tracker.try(&.capture(text)) || text
            text = WorkflowRun.capture(text, turn.workdir)
            @store.append_message(turn.chat_id, "tool", text)
            event_name = "tool"
            fields = {
              "chat"    => JSON::Any.new(turn.chat_id),
              "text"    => JSON::Any.new(text),
              "workdir" => JSON::Any.new(turn.workdir),
              "context" => JSON::Any.new(
                @worktree_service.describe(turn.workdir)
              ),
            }
          when EventType::Usage
            turn.context_used = event.context_used
            turn.context_window = event.context_window
          else
          end
        end

        publish(event_name, fields) if event_name
      rescue error : Storage::Error
        STDERR.puts "xd: cannot store agent event: #{error.message}"
      end

      private def finish(
        turn : ActiveTurn,
        success : Bool,
        message : String?,
      ) : Nil
        accepted = @mutex.synchronize do
          current = @turns[turn.chat_id]?
          next false unless current.same?(turn) && !turn.finished
          turn.finished = true
          true
        end
        return unless accepted

        if stale_resume?(turn, success)
          retry_stale(turn)
          return
        end

        asked = Ask.parse(turn.segment).try(&.ask)
        close_segment(turn)
        if success && !turn.had_text && !turn.had_tool
          @store.append_message(
            turn.chat_id,
            "assistant",
            "(no reply)",
            label: turn.label
          )
        end
        elapsed = Math.max(
          (@clock.call - turn.started_at).total_seconds.to_i64,
          0_i64
        )
        @store.append_message(turn.chat_id, "duration", elapsed.to_s)
        if turn.context_used > 0 && turn.context_window > 0
          @store.set_context_usage(
            turn.chat_id,
            turn.backend.id,
            turn.model,
            turn.context_used,
            turn.context_window
          )
        end
        error_text : String? = nil
        unless success
          error_text = if message.nil? || message.empty?
                         "The backend stopped unexpectedly."
                       else
                         message
                       end
          @store.append_message(turn.chat_id, "error", error_text)
        end
        last_message_id = @store.last_message_id(turn.chat_id)
        if success
          @store.set_last_seen(
            turn.chat_id,
            turn.backend.id,
            last_message_id
          )
        end

        next_text : String? = nil
        @mutex.synchronize do
          current = @turns[turn.chat_id]?
          @turns.delete(turn.chat_id) if current.same?(turn)
          unless @closed
            next_text = @store.queue_take_first(turn.chat_id)
            @starting << turn.chat_id if next_text
          end
        end

        fields = {
          "chat"    => JSON::Any.new(turn.chat_id),
          "ok"      => JSON::Any.new(success),
          "waiting" => JSON::Any.new(!asked.nil?),
          "silent"  => JSON::Any.new(
            success && !turn.had_text && !turn.had_tool
          ),
          "duration" => JSON::Any.new(elapsed),
          "last_message_id" => JSON::Any.new(last_message_id),
        }
        fields["error"] = JSON::Any.new(error_text) if error_text
        if ask = asked
          fields["question"] = JSON::Any.new(ask.question)
          fields["options"] = JSON::Any.new(
            ask.options.map { |option| JSON::Any.new(option) }
          )
          fields["accepts_input"] = JSON::Any.new(ask.accepts_input)
        end
        publish("turn-finished", fields)

        if text = next_text
          publish_queue(turn.chat_id)
          begin
            start_turn(turn.chat_id, text)
          rescue error : Error
            STDERR.puts "xd: cannot start queued turn: #{error.message}"
          end
        else
          @store.set_daemon_working(turn.chat_id, false)
        end
      rescue error : Storage::Error
        STDERR.puts "xd: cannot finish agent turn: #{error.message}"
      end

      private def stale_resume?(
        turn : ActiveTurn,
        success : Bool,
      ) : Bool
        !success &&
          turn.resumed &&
          !turn.retry_attempt &&
          !turn.had_text &&
          !turn.had_tool
      end

      private def retry_stale(turn : ActiveTurn) : Nil
        @store.set_session_id(turn.chat_id, turn.backend.id, nil)

        retrying = @mutex.synchronize do
          current = @turns[turn.chat_id]?
          next false unless current.same?(turn)

          @turns.delete(turn.chat_id)
          @starting << turn.chat_id
          true
        end
        return unless retrying

        start_turn(
          turn.chat_id,
          turn.prompt,
          user_submitted: false,
          retry_attempt: true
        )
      rescue error : Error
        text = error.message || "Cannot restart the agent"
        STDERR.puts "xd: cannot retry stale session: #{text}"
        publish("turn-finished", {
          "chat"    => JSON::Any.new(turn.chat_id),
          "ok"      => JSON::Any.new(false),
          "waiting" => JSON::Any.new(false),
          "error"   => JSON::Any.new(text),
        })
      end

      private def close_segment(turn : ActiveTurn) : Nil
        return if turn.segment.empty?

        if reported = WorkspaceBlock.parse(turn.segment)
          if selected = @worktree_service.registered_path(
               turn.workdir,
               reported.path
             )
            unless same_path?(turn.workdir, selected)
              @store.switch_workdir(
                turn.chat_id,
                selected,
                turn.workdir
              )
              turn.workdir = selected
              turn.diff_tracker = GitDiffTracker.open(selected)
            end
            turn.segment = reported.remainder
            if turn.segment_message_id != 0
              if turn.segment.empty?
                @store.delete_message(turn.segment_message_id)
              else
                @store.update_message(
                  turn.segment_message_id,
                  turn.segment
                )
              end
            end
          end
        end

        turn.segment = ""
        turn.segment_message_id = 0_i64
        turn.visible_segment_bytes = 0
      end

      private def same_path?(left : String, right : String) : Bool
        File.realpath(left) == File.realpath(right)
      rescue File::Error
        File.expand_path(left) == File.expand_path(right)
      end

      private def publish_queue(chat_id : String) : Nil
        queued = @store.get_chat(chat_id).queue
        fields = {
          "chat"  => JSON::Any.new(chat_id),
          "queue" => json_any(queued),
        }
        fields["text"] = JSON::Any.new(queued.first) unless queued.empty?
        publish("queued", fields)
      end

      private def publish(
        name : String?,
        fields = {} of String => JSON::Any,
      ) : Nil
        @on_event.call(name, fields) if name
      end

      private def json_any(value) : JSON::Any
        JSON.parse(value.to_json)
      end
    end
  end
end
