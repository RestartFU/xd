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

      @process : Process?

      def initialize(
        @checkout : String = BranchBuild.checkout_dir,
        @environment : Hash(String, String) = Agent::Environment.host,
        @command_builder : Proc(BranchBuild::Target, String, String) = ->(target : BranchBuild::Target, checkout : String) {
          BranchBuild.command(target, checkout)
        },
      )
        @lines = [] of String
        @trouble = nil
        @label = nil
        @process = nil
        @stopped = false
        @on_change = nil
        @on_installed = nil
      end

      def start(target : BranchBuild::Target) : Bool
        return false if @running

        command = @command_builder.call(target, @checkout)
        reader, writer = IO.pipe
        process = Process.new(
          ["sh", "-c", command],
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
        append_output("Fetching…")
        changed(false)

        output_done = Channel(Nil).new
        spawn read_output(reader, output_done)
        spawn finish_process(process, output_done)
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
        process = @process
        return unless process

        @stopped = true
        process.terminate(graceful: false)
      rescue RuntimeError
      end

      def clear_trouble : Nil
        return unless @trouble

        @trouble = nil
        changed(false)
      end

      def last_line : String
        @lines.last? || ""
      end

      def tail : String
        @lines.last(TAIL_LINES).join('\n').strip
      end

      private def read_output(
        reader : IO,
        done : Channel(Nil),
      ) : Nil
        while line = reader.gets
          append_output(line.rstrip)
        end
      rescue IO::Error
      ensure
        reader.close
        done.send(nil)
      end

      private def finish_process(
        process : Process,
        output_done : Channel(Nil),
      ) : Nil
        status = process.wait
        output_done.receive
        @running = false
        @process = nil
        @trouble = nil

        unless status.success?
          @trouble = if @stopped
                       "Stopped."
                     else
                       "#{@label || "That branch"} did not build."
                     end
          changed(false)
          return
        end

        @trouble = "Installed #{@label}. Restart to run it."
        @lines.clear
        @on_installed.try(&.call)
        changed(true)
      rescue RuntimeError | IO::Error
        @running = false
        @process = nil
        @trouble ||= "#{@label || "That branch"} did not build."
        changed(false)
      end

      private def append_output(line : String) : Nil
        @lines << line
        size = @lines.sum { |value| value.bytesize + 1 }
        while size > OUTPUT_LIMIT && @lines.size > 1
          removed = @lines.shift
          size -= removed.bytesize + 1
        end
        if size > OUTPUT_LIMIT
          if first = @lines.first?
            keep = first.byte_slice(
              Math.max(first.bytesize - OUTPUT_LIMIT, 0),
              Math.min(first.bytesize, OUTPUT_LIMIT)
            )
            @lines[0] = keep.scrub
          end
        end
      end

      private def changed(installed : Bool) : Nil
        @on_change.try(&.call(installed))
      end
    end
  end
end
