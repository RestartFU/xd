module Xd
  module Voice
    SAMPLE_RATE = 16_000_u32
    CHANNELS    =      1_u16

    MODEL_SIZE   = 147_964_211_u64
    MODEL_SHA256 =
      "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"

    class Error < Exception
    end

    module Data
      extend self

      def wav_from_s16(
        pcm : Bytes,
        sample_rate : UInt32 = SAMPLE_RATE,
        channels : UInt16 = CHANNELS,
      ) : Bytes
        raise Error.new("Audio sample rate must be positive.") if sample_rate == 0
        raise Error.new("Audio channel count must be positive.") if channels == 0
        if pcm.size > UInt32::MAX - 36
          raise Error.new("Recorded audio is too large.")
        end

        byte_rate = sample_rate.to_u64 * channels * 2
        if byte_rate > UInt32::MAX
          raise Error.new("Recorded audio format is too large.")
        end

        wav = Bytes.new(44 + pcm.size)
        put(wav, 0, "RIFF")
        put_u32(wav, 4, 36_u32 + pcm.size.to_u32)
        put(wav, 8, "WAVEfmt ")
        put_u32(wav, 16, 16_u32)
        put_u16(wav, 20, 1_u16)
        put_u16(wav, 22, channels)
        put_u32(wav, 24, sample_rate)
        put_u32(wav, 28, byte_rate.to_u32)
        put_u16(wav, 32, (channels * 2).to_u16)
        put_u16(wav, 34, 16_u16)
        put(wav, 36, "data")
        put_u32(wav, 40, pcm.size.to_u32)
        wav[44, pcm.size].copy_from(pcm)
        wav
      end

      def wav_to_f32(wav : Bytes) : Array(Float32)
        unless valid_header?(wav)
          raise Error.new("Recorded audio has an invalid WAV header.")
        end

        data_length = u32(wav, 40).to_i
        if data_length.odd? || data_length > wav.size - 44
          raise Error.new("Recorded audio data is truncated.")
        end

        Array(Float32).new(data_length // 2) do |index|
          encoded = u16(wav, 44 + index * 2).to_i32
          sample = encoded >= 0x8000 ? encoded - 0x10000 : encoded
          sample.to_f32 / 32768_f32
        end
      end

      def model_metadata_valid?(
        length : UInt64,
        sha256 : String,
      ) : Bool
        length == MODEL_SIZE && sha256 == MODEL_SHA256
      end

      private def valid_header?(wav : Bytes) : Bool
        wav.size >= 44 &&
          text(wav, 0, 4) == "RIFF" &&
          text(wav, 8, 8) == "WAVEfmt " &&
          u32(wav, 16) == 16 &&
          u16(wav, 20) == 1 &&
          u16(wav, 22) == CHANNELS &&
          u32(wav, 24) == SAMPLE_RATE &&
          u16(wav, 34) == 16 &&
          text(wav, 36, 4) == "data"
      end

      private def put(bytes : Bytes, offset : Int32, text : String) : Nil
        bytes[offset, text.bytesize].copy_from(text.to_slice)
      end

      private def put_u16(
        bytes : Bytes,
        offset : Int32,
        value : UInt16,
      ) : Nil
        bytes[offset] = (value & 0xff).to_u8
        bytes[offset + 1] = ((value >> 8) & 0xff).to_u8
      end

      private def put_u32(
        bytes : Bytes,
        offset : Int32,
        value : UInt32,
      ) : Nil
        4.times do |index|
          bytes[offset + index] =
            ((value >> (index * 8)) & 0xff).to_u8
        end
      end

      private def u16(bytes : Bytes, offset : Int32) : UInt16
        bytes[offset].to_u16 |
          (bytes[offset + 1].to_u16 << 8)
      end

      private def u32(bytes : Bytes, offset : Int32) : UInt32
        bytes[offset].to_u32 |
          (bytes[offset + 1].to_u32 << 8) |
          (bytes[offset + 2].to_u32 << 16) |
          (bytes[offset + 3].to_u32 << 24)
      end

      private def text(
        bytes : Bytes,
        offset : Int32,
        size : Int32,
      ) : String
        String.new(bytes[offset, size])
      end
    end
  end
end
