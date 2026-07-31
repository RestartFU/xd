require "base64"
require "uuid"
require "../agent/environment"

{% unless flag?(:win32) %}
  @[Link("util")]
  lib LibXdPty
    struct WinSize
      ws_row : UInt16
      ws_col : UInt16
      ws_xpixel : UInt16
      ws_ypixel : UInt16
    end

    fun forkpty(
      master : Int32*,
      name : UInt8*,
      termios : Void*,
      size : WinSize*,
    ) : LibC::PidT

    fun ioctl(fd : Int32, request : UInt64, argument : Void*) : Int32
  end
{% end %}

module Xd
  module Daemon
    record TerminalReplay,
      data : Bytes?,
      columns : Int32,
      rows : Int32

    class Terminal
      HISTORY_LIMIT       = 16 * 1024 * 1024
      INPUT_LIMIT         = 1 * 1024 * 1024
      REPLAY_ITEM_LIMIT   = 65_536
      DEFAULT_COLUMNS     =     80
      DEFAULT_ROWS        =     24
      MAX_GEOMETRY        =  1_000
      FORCE_CLOSE_DELAY   = 2.seconds
      REPLAY_LIMIT_NOTICE =
        "\r\n[xd: terminal closed after exceeding its replay limit]\r\n"

      class Error < Exception
      end

      getter id : String
      getter chat_id : String
      getter title : String

      @io : IO::FileDescriptor?
      @master = -1
      @pid = 0
      @reaped = false
      @columns : Int32
      @rows : Int32
      @replay = [] of TerminalReplay
      @replay_bytes = 0
      @pending_input = Deque(Bytes).new
      @pending_bytes = 0
      @input_ready = Channel(Nil).new(1)
      @lock = Mutex.new
      @closing = false
      @closed = false
      @started = false

      def initialize(
        @chat_id : String,
        workdir : String,
        columns : Int = DEFAULT_COLUMNS,
        rows : Int = DEFAULT_ROWS,
        @on_output : Proc(Terminal, Bytes, Nil) = ->(_terminal : Terminal, _data : Bytes) { },
        @on_closed : Proc(Terminal, Nil) = ->(_terminal : Terminal) { },
      )
        raise Error.new("A terminal needs a chat id.") if @chat_id.empty?
        unless File.directory?(workdir)
          raise Error.new("#{workdir} is not a directory.")
        end

        @id = UUID.random.to_s
        @title = File.basename(workdir)
        @columns = clamp_geometry(columns, DEFAULT_COLUMNS)
        @rows = clamp_geometry(rows, DEFAULT_ROWS)
        record_geometry(@columns, @rows)

        {% if flag?(:win32) %}
          raise Error.new("Terminal sessions are not supported on Windows yet.")
        {% else %}
          open_pty(workdir)
        {% end %}
      end

      def columns : Int32
        @lock.synchronize { @columns }
      end

      def rows : Int32
        @lock.synchronize { @rows }
      end

      def closing? : Bool
        @lock.synchronize { @closing || @closed }
      end

      def start : Nil
        start_now = @lock.synchronize do
          return if @started || @closing || @closed
          @started = true
        end
        return unless start_now

        spawn read_output
        spawn write_input
      end

      def write(data : Bytes) : Nil
        copy = data.dup
        should_signal = @lock.synchronize do
          raise Error.new("The terminal is closed.") if @closing || @closed
          if copy.size > INPUT_LIMIT - Math.min(@pending_bytes, INPUT_LIMIT)
            raise Error.new("The terminal input queue is full.")
          end

          was_empty = @pending_input.empty?
          @pending_input << copy
          @pending_bytes += copy.size
          was_empty
        end

        if should_signal
          @input_ready.send(nil)
        end
      rescue Channel::ClosedError
        raise Error.new("The terminal is closed.")
      end

      def resize(columns : Int, rows : Int) : {Int32, Int32}
        new_columns = clamp_geometry(columns, DEFAULT_COLUMNS)
        new_rows = clamp_geometry(rows, DEFAULT_ROWS)
        master = @lock.synchronize do
          raise Error.new("The terminal is closed.") if @closing || @closed
          if @replay.size >= REPLAY_ITEM_LIMIT
            raise Error.new("The terminal replay is full.")
          end
          @master
        end

        {% if flag?(:win32) %}
          raise Error.new("Terminal sessions are not supported on Windows yet.")
        {% else %}
          size = LibXdPty::WinSize.new
          size.ws_col = new_columns.to_u16
          size.ws_row = new_rows.to_u16
          request = {% if flag?(:darwin) %}
                      0x80087467_u64
                    {% else %}
                      0x5414_u64
                    {% end %}
          if LibXdPty.ioctl(
               master,
               request,
               pointerof(size).as(Void*)
             ) != 0
            raise Error.new("Cannot resize terminal: #{Errno.value.message}.")
          end
        {% end %}

        @lock.synchronize do
          raise Error.new("The terminal is closed.") if @closing || @closed
          @columns = new_columns
          @rows = new_rows
          record_geometry(new_columns, new_rows)
        end
        {new_columns, new_rows}
      end

      def replay_json : Array(JSON::Any)
        snapshot = @lock.synchronize { @replay.dup }
        snapshot.map do |item|
          if data = item.data
            JSON::Any.new({
              "data" => JSON::Any.new(Base64.strict_encode(data)),
            })
          else
            JSON::Any.new({
              "columns" => JSON::Any.new(item.columns.to_i64),
              "rows"    => JSON::Any.new(item.rows.to_i64),
            })
          end
        end
      end

      def close : Nil
        io = @lock.synchronize do
          return if @closing || @closed
          @closing = true
          @master = -1
          current = @io
          @io = nil
          current
        end

        begin
          io.try(&.close)
        rescue IO::Error
        end
        signal_session(LibC::SIGHUP) unless @pid == 0
        spawn finish_shutdown
      end

      private def clamp_geometry(value : Int, fallback : Int32) : Int32
        value = fallback if value == 0
        value.clamp(1, MAX_GEOMETRY).to_i32
      end

      private def record_geometry(columns : Int32, rows : Int32) : Nil
        if last = @replay.last?
          return if !last.data && last.columns == columns && last.rows == rows
        end
        if @replay.size >= REPLAY_ITEM_LIMIT
          raise Error.new("The terminal replay is full.")
        end
        @replay << TerminalReplay.new(nil, columns, rows)
      end

      {% unless flag?(:win32) %}
        private def open_pty(workdir : String) : Nil
          master = -1
          pid = 0
          begin
            size = LibXdPty::WinSize.new
            size.ws_col = @columns.to_u16
            size.ws_row = @rows.to_u16
            environment = Agent::Environment.host
            shell_name = environment["SHELL"]?.presence || "/bin/sh"
            shell = Process.find_executable(
              shell_name,
              environment["PATH"]?,
              workdir
            ) || shell_name
            environment["TERM"] = "xterm-256color"
            environment["COLORTERM"] = "truecolor"
            environment_entries = environment.map do |name, value|
              "#{name}=#{value}"
            end
            environment_pointers = environment_entries.map(&.to_unsafe)
            environment_pointers << Pointer(UInt8).null
            arguments = [
              shell.to_unsafe,
              Pointer(UInt8).null,
            ]
            pid = LibXdPty.forkpty(
              pointerof(master),
              Pointer(UInt8).null,
              Pointer(Void).null,
              pointerof(size)
            )
            if pid < 0
              raise Error.new(
                "Cannot open a terminal: #{Errno.value.message}."
              )
            end

            if pid == 0
              LibC.chdir(workdir)
              LibC.execve(
                shell.to_unsafe,
                arguments.to_unsafe,
                environment_pointers.to_unsafe
              )
              LibC._exit(127)
            end

            @pid = pid
            @master = master
            @io = IO::FileDescriptor.new(master)
            @io.not_nil!.close_on_exec = true
          rescue error
            if pid > 0
              LibC.kill(-pid, LibC::SIGKILL)
              LibC.kill(pid, LibC::SIGKILL)
              LibC.waitpid(pid, Pointer(Int32).null, 0)
            end
            LibC.close(master) if master >= 0
            raise error
          end
        end
      {% end %}

      private def read_output : Nil
        io = @lock.synchronize { @io }
        return unless io

        buffer = Bytes.new(8192)
        loop do
          count = io.read(buffer)
          break if count == 0
          break unless record_output(buffer[0, count].dup)
          Fiber.yield
        end
      rescue IO::Error
      ensure
        natural_exit unless @lock.synchronize { @closing }
      end

      private def record_output(data : Bytes) : Bool
        accepted = @lock.synchronize do
          return false if @closing || @closed
          if data.size > HISTORY_LIMIT - @replay_bytes ||
             @replay.size >= REPLAY_ITEM_LIMIT
            false
          else
            @replay << TerminalReplay.new(data, 0, 0)
            @replay_bytes += data.size
            true
          end
        end

        if accepted
          @on_output.call(self, data)
        else
          @on_output.call(self, REPLAY_LIMIT_NOTICE.to_slice)
          close
        end
        accepted
      end

      private def write_input : Nil
        loop do
          @input_ready.receive
          while data = next_input
            master = @lock.synchronize { @master }
            break if master < 0
            write_all(master, data)
            @lock.synchronize do
              @pending_bytes -= data.size
            end
            Fiber.yield
          end
        end
      rescue Channel::ClosedError
      rescue IO::Error
        close
      end

      private def next_input : Bytes?
        @lock.synchronize { @pending_input.shift? }
      end

      private def write_all(master : Int32, data : Bytes) : Nil
        offset = 0
        while offset < data.size
          count = LibC.write(
            master,
            data.to_unsafe + offset,
            data.size - offset
          )
          if count > 0
            offset += count.to_i
            next
          end

          error = Errno.value
          if error == Errno::EINTR
            next
          elsif error == Errno::EAGAIN || error == Errno::EWOULDBLOCK
            sleep 5.milliseconds
            next
          end
          raise IO::Error.new("Cannot write terminal: #{error.message}")
        end
      end

      private def natural_exit : Nil
        reap_child
        if session_alive?
          @lock.synchronize { @closing = true }
          signal_session(LibC::SIGHUP)
          spawn finish_shutdown
        else
          finish
        end
      end

      private def finish_shutdown : Nil
        deadline = Time.instant + FORCE_CLOSE_DELAY
        while session_alive? && Time.instant < deadline
          reap_child
          sleep 20.milliseconds
        end
        signal_session(LibC::SIGKILL) if session_alive?

        50.times do
          reap_child
          break unless session_alive?
          sleep 20.milliseconds
        end
        finish
      end

      private def reap_child : Nil
        return if @pid == 0 || @reaped
        result = LibC.waitpid(@pid, Pointer(Int32).null, LibC::WNOHANG)
        @reaped = true if result == @pid
      end

      private def session_alive? : Bool
        return false if @pid == 0

        {% if flag?(:linux) %}
          found = false
          begin
            Dir.each_child("/proc") do |name|
              next unless name.to_i? && process_session(name) == @pid
              found = true
              break
            end
          rescue File::Error
            found = LibC.kill(-@pid, 0) == 0
          end
          found
        {% else %}
          LibC.kill(-@pid, 0) == 0
        {% end %}
      end

      private def signal_session(signal : Int32) : Nil
        {% if flag?(:linux) %}
          signaled = false
          begin
            Dir.each_child("/proc") do |name|
              process = name.to_i?
              next unless process && process_session(name) == @pid
              LibC.kill(process, signal)
              signaled = true
            end
          rescue File::Error
          end
          LibC.kill(-@pid, signal) unless signaled
        {% else %}
          LibC.kill(-@pid, signal)
        {% end %}
        LibC.kill(@pid, signal)
      end

      private def process_session(name : String) : Int32?
        stat = File.read(File.join("/proc", name, "stat"))
        closing = stat.rindex(')')
        return unless closing

        fields = stat.byte_slice(closing + 1).split
        fields[3]?.try(&.to_i?)
      rescue File::Error
        nil
      end

      private def finish : Nil
        callback = @lock.synchronize do
          return if @closed
          @closed = true
          @closing = true
          @master = -1
          @io = nil
          @input_ready.close
          @on_closed
        end
        callback.call(self)
      end
    end
  end
end
