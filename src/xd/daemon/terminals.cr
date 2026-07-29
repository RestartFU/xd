require "base64"
require "json"
require "./filesystem"
require "./terminal"

module Xd
  module Daemon
    record TerminalOpenResult, terminal : Terminal, created : Bool

    class Terminals
      class Error < Exception
      end

      @terminals = {} of String => Terminal
      @lock = Mutex.new

      def initialize(
        @filesystem : Filesystem,
        @on_event : Proc(String, Hash(String, JSON::Any), Nil),
      )
      end

      def list(chat_id : String) : Array(JSON::Any)
        terminals = @lock.synchronize { @terminals.values.dup }
        terminals
          .reject { |terminal|
            terminal.chat_id != chat_id || terminal.closing?
          }
          .map { |terminal| terminal_json(terminal) }
      end

      def open(
        chat_id : String,
        columns : Int,
        rows : Int,
        reuse : Bool,
      ) : TerminalOpenResult
        if reuse
          existing = @lock.synchronize do
            @terminals.values.find do |terminal|
              terminal.chat_id == chat_id && !terminal.closing?
            end
          end
          return TerminalOpenResult.new(existing, false) if existing
        end

        terminal = Terminal.new(
          chat_id,
          @filesystem.workdir(chat_id),
          columns,
          rows,
          ->(source : Terminal, data : Bytes) {
            output(source, data)
          },
          ->(source : Terminal) {
            closed(source)
          }
        )
        @lock.synchronize { @terminals[terminal.id] = terminal }
        TerminalOpenResult.new(terminal, true)
      rescue error : Terminal::Error
        raise Error.new(error.message || "Cannot open terminal.")
      end

      def start(id : String) : Nil
        terminal(id).start
      end

      def input(id : String, encoded : String) : Nil
        data = Base64.decode(encoded)
        terminal(id).write(data)
      rescue Base64::Error
        raise Error.new("terminal-input needs valid base64 data.")
      rescue error : Terminal::Error
        raise Error.new(error.message || "Cannot write terminal.")
      end

      def resize(
        id : String,
        columns : Int,
        rows : Int,
      ) : {Terminal, Int32, Int32}
        source = terminal(id)
        width, height = source.resize(columns, rows)
        {source, width, height}
      rescue error : Terminal::Error
        raise Error.new(error.message || "Cannot resize terminal.")
      end

      def kill(id : String) : Nil
        source = @lock.synchronize { @terminals[id]? }
        source.try(&.close)
      end

      def kill_chat(chat_id : String) : Nil
        sources = @lock.synchronize do
          @terminals.values.select(&.chat_id.==(chat_id))
        end
        sources.each(&.close)
      end

      def close : Nil
        sources = @lock.synchronize { @terminals.values.dup }
        sources.each(&.close)

        deadline = Time.instant + 3.seconds
        while Time.instant < deadline
          break if @lock.synchronize { @terminals.empty? }
          sleep 10.milliseconds
        end
      end

      private def terminal(id : String) : Terminal
        @lock.synchronize { @terminals[id]? } ||
          raise Error.new("No such terminal.")
      end

      private def output(terminal : Terminal, data : Bytes) : Nil
        @on_event.call("terminal-output", {
          "chat"     => JSON::Any.new(terminal.chat_id),
          "terminal" => JSON::Any.new(terminal.id),
          "data"     => JSON::Any.new(Base64.strict_encode(data)),
        })
      end

      private def closed(terminal : Terminal) : Nil
        @on_event.call("terminal-closed", {
          "chat"     => JSON::Any.new(terminal.chat_id),
          "terminal" => JSON::Any.new(terminal.id),
        })
        @lock.synchronize { @terminals.delete(terminal.id) }
      end

      private def terminal_json(terminal : Terminal) : JSON::Any
        JSON::Any.new({
          "id"      => JSON::Any.new(terminal.id),
          "title"   => JSON::Any.new(terminal.title),
          "columns" => JSON::Any.new(terminal.columns.to_i64),
          "rows"    => JSON::Any.new(terminal.rows.to_i64),
          "replay"  => JSON::Any.new(terminal.replay_json),
        })
      end
    end
  end
end
