require "../version"
require "./data"

@[Link("pulse-simple")]
lib LibPulseSimple
  struct SampleSpec
    format : Int32
    rate : UInt32
    channels : UInt8
  end

  fun create = pa_simple_new(
    server : UInt8*,
    name : UInt8*,
    direction : Int32,
    device : UInt8*,
    stream_name : UInt8*,
    sample_spec : SampleSpec*,
    channel_map : Void*,
    buffer_attributes : Void*,
    error : Int32*,
  ) : Void*
  fun read = pa_simple_read(
    stream : Void*,
    data : Void*,
    bytes : LibC::SizeT,
    error : Int32*,
  ) : Int32
  fun free = pa_simple_free(stream : Void*) : Void
end

@[Link("pulse")]
lib LibPulse
  fun error_string = pa_strerror(error : Int32) : UInt8*
end

module Xd
  module Voice
    record Recording,
      wav : Bytes?,
      error : String?,
      cancelled : Bool

    class Recorder
      CHUNK_MILLISECONDS = 100
      MAX_SECONDS        = 120
      MIN_BYTES          = SAMPLE_RATE.to_i * 2 // 4
      CHUNK_BYTES        =
        SAMPLE_RATE.to_i * CHANNELS.to_i * 2 *
          CHUNK_MILLISECONDS // 1000
      MAX_BYTES =
        SAMPLE_RATE.to_i * CHANNELS.to_i * 2 * MAX_SECONDS

      def initialize
        @stop_requested = Atomic(Bool).new(false)
        @cancelled = Atomic(Bool).new(false)
        @running = Atomic(Bool).new(false)
      end

      def record(&finished : Recording -> Nil) : Nil
        unless @running.compare_and_set(false, true)[1]
          raise Error.new("Voice recording is already running.")
        end

        @stop_requested.set(false)
        @cancelled.set(false)
        Thread.new do
          result = record_blocking
          @running.set(false)
          finished.call(result)
        end
      end

      def stop : Nil
        @stop_requested.set(true)
      end

      def cancel : Nil
        @cancelled.set(true)
        @stop_requested.set(true)
      end

      private def record_blocking : Recording
        error_code = 0
        samples = LibPulseSimple::SampleSpec.new(
          format: 3,
          rate: SAMPLE_RATE,
          channels: CHANNELS.to_u8
        )
        stream = LibPulseSimple.create(
          Pointer(UInt8).null,
          APP_NAME,
          2,
          Pointer(UInt8).null,
          "Voice prompt",
          pointerof(samples),
          Pointer(Void).null,
          Pointer(Void).null,
          pointerof(error_code)
        )
        if stream.null?
          return Recording.new(
            nil,
            "Cannot open microphone: #{pulse_error(error_code)}",
            false
          )
        end

        pcm = IO::Memory.new
        chunk = Bytes.new(CHUNK_BYTES)
        begin
          while !@stop_requested.get && pcm.size < MAX_BYTES
            if LibPulseSimple.read(
                 stream,
                 chunk.to_unsafe,
                 chunk.size,
                 pointerof(error_code)
               ) < 0
              return Recording.new(
                nil,
                "Cannot record microphone: #{pulse_error(error_code)}",
                false
              )
            end
            pcm.write(chunk)
          end
        ensure
          LibPulseSimple.free(stream)
        end

        return Recording.new(nil, nil, true) if @cancelled.get
        if pcm.size < MIN_BYTES
          return Recording.new(nil, "Recording was too short.", false)
        end
        Recording.new(
          Data.wav_from_s16(pcm.to_slice),
          nil,
          false
        )
      rescue error
        Recording.new(
          nil,
          error.message || "Cannot record microphone.",
          @cancelled.get
        )
      end

      private def pulse_error(code : Int32) : String
        pointer = LibPulse.error_string(code)
        pointer.null? ? "unknown PulseAudio error" : String.new(pointer)
      end
    end
  end
end
