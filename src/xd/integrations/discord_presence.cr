require "json"
require "socket"

module Xd
  # Best-effort Discord Rich Presence. All pipe/socket work owns an isolated
  # system thread: Discord being absent, wedged, or closed can never stall GTK
  # or the daemon scheduler.
  class DiscordPresence
    APPLICATION_ID = "1531361363522490489"
    DEFAULT_STATE  = "Browsing workspaces"
    DETAILS        = "Building with AI"
    MAX_FRAME      = 1024 * 1024
    IO_TIMEOUT     = 2.seconds
    RETRY_INTERVAL = 15.seconds
    POLL_INTERVAL  = 250.milliseconds

    private enum Opcode : UInt32
      Handshake = 0
      Frame     = 1
      Close     = 2
      Ping      = 3
      Pong      = 4
    end

    @lock = Mutex.new
    @state = DEFAULT_STATE
    @version = 1_u64
    @closed = false
    @started_at = Time.utc.to_unix

    def initialize
      Fiber::ExecutionContext::Isolated.new("xd discord presence") do
        run
      end
    rescue RuntimeError
      # Presence is optional. Thread-pool exhaustion must not stop the app.
    end

    def state=(state : String) : Nil
      return if state.empty?

      @lock.synchronize do
        return if @closed || @state == state

        @state = state
        @version &+= 1
      end
    end

    def close : Nil
      @lock.synchronize do
        return if @closed

        @closed = true
        @version &+= 1
      end
    end

    def self.activity(
      state : String,
      started_at : Int64,
      process_id : Int64,
      nonce : UInt64,
    ) : String
      {
        "cmd"  => "SET_ACTIVITY",
        "args" => {
          "pid"      => process_id,
          "activity" => {
            "details"    => DETAILS,
            "state"      => state,
            "timestamps" => {
              "start" => started_at,
            },
          },
        },
        "nonce" => nonce.to_s,
      }.to_json
    end

    def self.handshake : String
      {
        "v"         => 1,
        "client_id" => APPLICATION_ID,
      }.to_json
    end

    def self.clear_activity(
      process_id : Int64,
      nonce : UInt64,
    ) : String
      {
        "cmd"  => "SET_ACTIVITY",
        "args" => {
          "pid"      => process_id,
          "activity" => nil,
        },
        "nonce" => nonce.to_s,
      }.to_json
    end

    private def run : Nil
      connection : IO? = nil
      sent_version = 0_u64
      nonce = 1_u64
      next_retry = Time.instant
      next_refresh = Time.instant

      loop do
        state, version, closed = snapshot
        if closed
          if active = connection
            send_frame(
              active,
              Opcode::Frame,
              self.class.clear_activity(Process.pid.to_i64, nonce)
            )
          end
          break
        end

        unless connection
          if Time.instant >= next_retry
            connection = connect
            if active = connection
              if send_frame(active, Opcode::Handshake, self.class.handshake) &&
                 read_reply(active)
                sent_version = 0_u64
                next_refresh = Time.instant
              else
                close(active)
                connection = nil
                next_retry = Time.instant + RETRY_INTERVAL
              end
            else
              next_retry = Time.instant + RETRY_INTERVAL
            end
          end
        end

        if active = connection
          if version != sent_version || Time.instant >= next_refresh
            payload = self.class.activity(
              state,
              @started_at,
              Process.pid.to_i64,
              nonce
            )
            nonce &+= 1
            if send_frame(active, Opcode::Frame, payload) &&
               read_reply(active)
              sent_version = version
              next_refresh = Time.instant + RETRY_INTERVAL
            else
              close(active)
              connection = nil
              next_retry = Time.instant + RETRY_INTERVAL
            end
          end
        end

        sleep POLL_INTERVAL
      end
    rescue error
      STDERR.puts "xd: Discord presence stopped: #{error.message}"
    ensure
      close(connection)
    end

    private def snapshot : {String, UInt64, Bool}
      @lock.synchronize { {@state, @version, @closed} }
    end

    private def connect : IO?
      {% if flag?(:win32) %}
        10.times do |index|
          begin
            return File.open(
              "\\\\?\\pipe\\discord-ipc-#{index}",
              "r+"
            )
          rescue File::Error
          end
        end
      {% else %}
        directories = [
          ENV["XDG_RUNTIME_DIR"]?,
          ENV["TMPDIR"]?,
          ENV["TMP"]?,
          ENV["TEMP"]?,
          "/tmp",
        ].compact.uniq
        directories.each do |directory|
          10.times do |index|
            socket : UNIXSocket? = nil
            begin
              socket = UNIXSocket.new(
                File.join(directory, "discord-ipc-#{index}")
              )
              socket.read_timeout = IO_TIMEOUT
              socket.write_timeout = IO_TIMEOUT
              return socket
            rescue IO::Error
              socket.try(&.close)
            end
          end
        end
      {% end %}
      nil
    end

    private def send_frame(
      connection : IO,
      opcode : Opcode,
      payload : String,
    ) : Bool
      return false if payload.bytesize > MAX_FRAME

      header = IO::Memory.new(8)
      header.write_bytes(opcode.value, IO::ByteFormat::LittleEndian)
      header.write_bytes(
        payload.bytesize.to_u32,
        IO::ByteFormat::LittleEndian
      )
      connection.write(header.to_slice)
      connection << payload
      connection.flush
      true
    rescue IO::Error | OverflowError
      false
    end

    private def read_frame(connection : IO) : {Opcode, String}?
      header = Bytes.new(8)
      connection.read_fully(header)
      input = IO::Memory.new(header)
      opcode = Opcode.from_value(
        input.read_bytes(UInt32, IO::ByteFormat::LittleEndian)
      )
      length = input.read_bytes(UInt32, IO::ByteFormat::LittleEndian)
      return if length > MAX_FRAME

      payload = Bytes.new(length.to_i)
      connection.read_fully(payload)
      {opcode, String.new(payload)}
    rescue IO::Error | ArgumentError | OverflowError
      nil
    end

    private def read_reply(connection : IO) : Bool
      8.times do
        opcode, payload = read_frame(connection) || return false
        if opcode.ping?
          return false unless send_frame(connection, Opcode::Pong, payload)
          next
        end
        return false unless opcode.frame?

        root = JSON.parse(payload).as_h?
        return false unless root
        return root["evt"]?.try(&.as_s?) != "ERROR"
      rescue JSON::ParseException
        return false
      end
      false
    end

    private def close(connection : IO?) : Nil
      connection.try(&.close)
    rescue IO::Error
    end
  end
end
