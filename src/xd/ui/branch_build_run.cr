require "../agent/environment"
require "./branch_build"

module Xd
  module UI
    class BranchBuildRun
      OUTPUT_LIMIT = 8 * 1024
      TAIL_LINES   = 8

      getter running = false
      getter trouble : String?
      getter label : String?
      property on_change : Proc(Bool, Nil)?
      property on_installed : Proc(Nil)?

      def initialize(
        @checkout : String = BranchBuild.checkout_dir,
        @environment : Hash(String, String) = Agent::Environment.host,
        @command_builder : Proc(BranchBuild::Target, String, String) = ->(target : BranchBuild::Target, checkout : String) { BranchBuild.command(target, checkout) },
      )
        @lines = [] of String
        @process = nil.as(Process?)
        @stopped = false
        @lines_mutex = Mutex.new
      end

      def start(target : BranchBuild::Target) : Bool
        return false if @running || !BranchBuild.supported?
        reader, writer = IO.pipe
        process = Process.new(
          ["sh", "-c", @command_builder.call(target, @checkout)],
          env: @environment,
          clear_env: true,
          input: Process::Redirect::Close,
          output: writer,
          error: writer
        )
        writer.close
        @process = process
        @label = target.label
        @trouble = nil
        @lines.clear
        @stopped = false
        @running = true
        append("Fetching…")
        changed(false)
        done = Channel(Nil).new
        spawn read_output(reader, done)
        spawn finish(process, done)
        true
      rescue error : File::Error | IO::Error
        writer.try(&.close)
        reader.try(&.close)
        @trouble = error.message || "Cannot start the build."
        @running = false
        changed(false)
        false
      end

      def stop : Nil
        return unless @running
        @stopped = true
        @process.try(&.terminate(graceful: false))
      rescue RuntimeError
      end

      def clear_trouble : Nil
        @trouble = nil
      end

      def last_line : String
        @lines_mutex.synchronize { @lines.last? || "" }
      end

      def tail : String
        @lines_mutex.synchronize { @lines.last(TAIL_LINES).join('\n').strip }
      end

      private def read_output(reader : IO, done : Channel(Nil)) : Nil
        while line = reader.gets
          append(line.rstrip)
          Fiber.yield
        end
      rescue IO::Error
      ensure
        reader.close
        done.send(nil)
      end

      private def finish(process : Process, done : Channel(Nil)) : Nil
        status = process.wait
        done.receive
        @running = false
        @process = nil
        if status.success?
          @trouble = "Installed #{@label}. Restart XD to run it."
          @lines.clear
          @on_installed.try(&.call)
          changed(true)
        else
          @trouble = @stopped ? "Stopped." : "#{@label || "Source"} did not build."
          changed(false)
        end
      rescue RuntimeError | IO::Error
        @running = false
        @process = nil
        @trouble ||= "#{@label || "Source"} did not build."
        changed(false)
      end

      private def append(line : String) : Nil
        @lines_mutex.synchronize do
          @lines << line
          while @lines.sum { |item| item.bytesize + 1 } > OUTPUT_LIMIT && @lines.size > 1
            @lines.shift
          end
        end
      end

      private def changed(installed : Bool) : Nil
        @on_change.try(&.call(installed))
      end
    end
  end
end
