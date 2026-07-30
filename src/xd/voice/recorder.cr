require "../version"
require "./data"

{% if flag?(:darwin) || flag?(:win32) || flag?(:xd_portaudio_backend) %}
  {% if flag?(:win32) %}
    @[Link("portaudio", dll: "libportaudio.dll")]
  {% else %}
    @[Link("portaudio")]
  {% end %}
  lib LibPortAudio
    PA_NO_ERROR         =     0
    PA_INPUT_OVERFLOWED = -9981
    PA_INT16            =     8

    fun version = Pa_GetVersion : Int32
    fun initialize = Pa_Initialize : Int32
    fun terminate = Pa_Terminate : Int32
    fun open_default_stream = Pa_OpenDefaultStream(
      stream : Void**,
      input_channels : Int32,
      output_channels : Int32,
      sample_format : LibC::ULong,
      sample_rate : Float64,
      frames_per_buffer : LibC::ULong,
      callback : Void*,
      user_data : Void*,
    ) : Int32
    fun start_stream = Pa_StartStream(stream : Void*) : Int32
    fun stop_stream = Pa_StopStream(stream : Void*) : Int32
    fun close_stream = Pa_CloseStream(stream : Void*) : Int32
    fun read_stream = Pa_ReadStream(
      stream : Void*,
      buffer : Void*,
      frames : LibC::ULong,
    ) : Int32
    fun error_text = Pa_GetErrorText(error : Int32) : UInt8*
  end
{% else %}
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
{% end %}

module Xd
  module Voice
    record Recording,
      wav : Bytes?,
      error : String?,
      cancelled : Bool

    class Recorder
      BACKEND = {% if flag?(:darwin) || flag?(:win32) || flag?(:xd_portaudio_backend) %}
                  :portaudio
                {% else %}
                  :pulse
                {% end %}

      CHUNK_MILLISECONDS = 100
      MAX_SECONDS        = 120
      MIN_BYTES          = SAMPLE_RATE.to_i * 2 // 4
      CHUNK_FRAMES       =
        SAMPLE_RATE.to_i * CHUNK_MILLISECONDS // 1000
      CHUNK_BYTES =
        CHUNK_FRAMES * CHANNELS.to_i * 2
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
        Fiber::ExecutionContext::Isolated.new("xd voice recorder") do
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
        pcm = {% if flag?(:darwin) || flag?(:win32) || flag?(:xd_portaudio_backend) %}
                record_portaudio
              {% else %}
                record_pulse
              {% end %}

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

      {% if flag?(:darwin) || flag?(:win32) || flag?(:xd_portaudio_backend) %}
        private def record_portaudio : IO::Memory
          code = LibPortAudio.initialize
          unless code == LibPortAudio::PA_NO_ERROR
            raise Error.new(
              "Cannot initialize microphone: #{portaudio_error(code)}"
            )
          end

          stream = Pointer(Void).null
          started = false
          begin
            code = LibPortAudio.open_default_stream(
              pointerof(stream),
              CHANNELS.to_i,
              0,
              LibC::ULong.new(LibPortAudio::PA_INT16),
              SAMPLE_RATE.to_f64,
              LibC::ULong.new(CHUNK_FRAMES),
              Pointer(Void).null,
              Pointer(Void).null
            )
            unless code == LibPortAudio::PA_NO_ERROR
              raise Error.new(
                "Cannot open microphone: #{portaudio_error(code)}"
              )
            end

            code = LibPortAudio.start_stream(stream)
            unless code == LibPortAudio::PA_NO_ERROR
              raise Error.new(
                "Cannot start microphone: #{portaudio_error(code)}"
              )
            end
            started = true

            pcm = IO::Memory.new
            chunk = Bytes.new(CHUNK_BYTES)
            while !@stop_requested.get && pcm.size < MAX_BYTES
              code = LibPortAudio.read_stream(
                stream,
                chunk.to_unsafe,
                LibC::ULong.new(CHUNK_FRAMES)
              )
              unless code == LibPortAudio::PA_NO_ERROR ||
                     code == LibPortAudio::PA_INPUT_OVERFLOWED
                raise Error.new(
                  "Cannot record microphone: #{portaudio_error(code)}"
                )
              end
              pcm.write(chunk)
            end
            pcm
          ensure
            unless stream.null?
              LibPortAudio.stop_stream(stream) if started
              LibPortAudio.close_stream(stream)
            end
            LibPortAudio.terminate
          end
        end

        private def portaudio_error(code : Int32) : String
          pointer = LibPortAudio.error_text(code)
          pointer.null? ? "unknown PortAudio error" : String.new(pointer)
        end
      {% else %}
        private def record_pulse : IO::Memory
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
            raise Error.new(
              "Cannot open microphone: #{pulse_error(error_code)}"
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
                raise Error.new(
                  "Cannot record microphone: #{pulse_error(error_code)}"
                )
              end
              pcm.write(chunk)
            end
          ensure
            LibPulseSimple.free(stream)
          end
          pcm
        end

        private def pulse_error(code : Int32) : String
          pointer = LibPulse.error_string(code)
          pointer.null? ? "unknown PulseAudio error" : String.new(pointer)
        end
      {% end %}
    end
  end
end
