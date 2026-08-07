package com.restartfu.xd.voice

/**
 * The one audio shape the daemon accepts: 16 kHz mono 16-bit PCM in a WAV
 * container.
 *
 * `daemon-rs/src/voice.rs` validates this header field by field, so the phone
 * has to produce the same 44 bytes rather than
 * whatever an encoder feels like emitting. Whisper wants 16 kHz mono, and the
 * daemon does not resample.
 */
public object Wav {
    public const val SAMPLE_RATE: Int = 16_000
    public const val CHANNELS: Int = 1
    public const val BITS_PER_SAMPLE: Int = 16
    private const val BYTES_PER_SAMPLE: Int = BITS_PER_SAMPLE / 8
    public const val HEADER_BYTES: Int = 44

    /** The desktop recorder's ceiling, so both clients refuse at the same length. */
    public const val MAX_SECONDS: Int = 120
    public const val MAX_PCM_BYTES: Int = SAMPLE_RATE * CHANNELS * BYTES_PER_SAMPLE * MAX_SECONDS

    /** A quarter second. Below this there is nothing to transcribe. */
    public const val MIN_PCM_BYTES: Int = SAMPLE_RATE * BYTES_PER_SAMPLE / 4

    /**
     * Wraps little-endian PCM16 samples in a WAV header.
     *
     * The samples are copied through untouched: Android's `AudioRecord` already
     * hands back little-endian PCM16, which is what WAV stores.
     */
    public fun fromPcm16(pcm: ByteArray): ByteArray {
        require(pcm.isNotEmpty()) { "There is no audio to send" }
        require(pcm.size % (CHANNELS * BYTES_PER_SAMPLE) == 0) {
            "PCM data must be whole samples"
        }

        val byteRate = SAMPLE_RATE * CHANNELS * BYTES_PER_SAMPLE
        val wav = ByteArray(HEADER_BYTES + pcm.size)
        wav.ascii(0, "RIFF")
        wav.u32(4, 36 + pcm.size)
        wav.ascii(8, "WAVEfmt ")
        // A 16-byte fmt chunk describing format 1, which is uncompressed PCM.
        wav.u32(16, 16)
        wav.u16(20, 1)
        wav.u16(22, CHANNELS)
        wav.u32(24, SAMPLE_RATE)
        wav.u32(28, byteRate)
        wav.u16(32, CHANNELS * BYTES_PER_SAMPLE)
        wav.u16(34, BITS_PER_SAMPLE)
        wav.ascii(36, "data")
        wav.u32(40, pcm.size)
        pcm.copyInto(wav, HEADER_BYTES)
        return wav
    }

    private fun ByteArray.ascii(offset: Int, text: String) {
        text.forEachIndexed { index, ch -> this[offset + index] = ch.code.toByte() }
    }

    private fun ByteArray.u16(offset: Int, value: Int) {
        this[offset] = (value and 0xFF).toByte()
        this[offset + 1] = ((value ushr 8) and 0xFF).toByte()
    }

    private fun ByteArray.u32(offset: Int, value: Int) {
        repeat(4) { index ->
            this[offset + index] = ((value ushr (index * 8)) and 0xFF).toByte()
        }
    }
}
