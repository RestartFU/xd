require "base64"
require "json"

module Xd
  module UI
    record TerminalReplayGeometry,
      columns : Int64,
      rows : Int64

    alias TerminalReplayAction = Bytes | TerminalReplayGeometry
    alias TerminalReplayPending = String | Bytes | TerminalReplayGeometry

    # Lazily decodes terminal history in bounded batches. Keeping JSON parsing,
    # Base64 work and VTE feeds below one batch budget lets GTK process input
    # between chunks even when a daemon returns its full 16 MiB history.
    class TerminalReplay
      BATCH_ITEMS           = 64
      BATCH_DECODED_BYTES   = 32 * 1024
      MAX_ENCODED_ITEM      = 128 * 1024
      INVALID_REPLAY_NOTICE =
        "\r\n[xd: skipped invalid terminal replay data]\r\n"

      @position = 0
      @pending = Deque(TerminalReplayPending).new
      @deferred : JSON::Any | TerminalReplayPending | Nil = nil
      @mutex = Mutex.new

      def initialize(
        @items : Array(JSON::Any),
        @fallback_columns : Int64,
        @fallback_rows : Int64,
      )
      end

      def append_data(encoded : String) : Nil
        @mutex.synchronize { @pending << encoded }
      end

      def append_geometry(columns : Int64, rows : Int64) : Nil
        @mutex.synchronize do
          @pending << TerminalReplayGeometry.new(columns, rows)
        end
      end

      def next_batch : Array(TerminalReplayAction)
        @mutex.synchronize do
          actions = [] of TerminalReplayAction
          decoded_bytes = 0
          BATCH_ITEMS.times do
            input = next_input
            break unless input

            estimate = estimated_decoded_bytes(input)
            if !actions.empty? &&
               decoded_bytes + estimate > BATCH_DECODED_BYTES
              @deferred = input
              break
            end
            action = decode(input)
            if action.is_a?(Bytes) &&
               action.size > BATCH_DECODED_BYTES - decoded_bytes
              available = BATCH_DECODED_BYTES - decoded_bytes
              actions << action[0, available]
              @deferred = action[available, action.size - available]
              break
            end
            actions << action
            decoded_bytes += action.size if action.is_a?(Bytes)
            break if decoded_bytes >= BATCH_DECODED_BYTES
          end
          actions
        end
      end

      def done? : Bool
        @mutex.synchronize do
          @deferred.nil? &&
            @position >= @items.size &&
            @pending.empty?
        end
      end

      private def next_input : JSON::Any | TerminalReplayPending | Nil
        if input = @deferred
          @deferred = nil
          return input
        end
        if @position < @items.size
          item = @items[@position]
          @position += 1
          item
        else
          @pending.shift?
        end
      end

      private def estimated_decoded_bytes(
        input : JSON::Any | TerminalReplayPending,
      ) : Int32
        encoded = if input.is_a?(String)
                    input
                  elsif input.is_a?(Bytes)
                    return input.size
                  elsif input.is_a?(TerminalReplayGeometry)
                    return 0
                  else
                    input.as_h?.try(&.["data"]?).try(&.as_s?)
                  end
        return 0 unless encoded
        Math.min(
          encoded.bytesize.to_i64 * 3 // 4,
          MAX_ENCODED_ITEM.to_i64 * 3 // 4
        ).to_i32
      end

      private def decode(
        input : JSON::Any | TerminalReplayPending,
      ) : TerminalReplayAction
        if input.is_a?(String)
          return decode_data(input)
        elsif input.is_a?(Bytes)
          return input
        elsif input.is_a?(TerminalReplayGeometry)
          return input
        end

        object = input.as_h?
        return invalid_notice unless object
        if encoded = object["data"]?.try(&.as_s?)
          decode_data(encoded)
        else
          TerminalReplayGeometry.new(
            object["columns"]?.try(&.as_i64?) || @fallback_columns,
            object["rows"]?.try(&.as_i64?) || @fallback_rows
          )
        end
      end

      private def decode_data(encoded : String) : Bytes
        return invalid_notice if encoded.bytesize > MAX_ENCODED_ITEM
        Base64.decode(encoded)
      rescue Base64::Error
        invalid_notice
      end

      private def invalid_notice : Bytes
        INVALID_REPLAY_NOTICE.to_slice
      end
    end
  end
end
